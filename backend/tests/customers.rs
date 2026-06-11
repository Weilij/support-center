//! Customers directory & customer-tag associations per Rust_CRD.md §3.1
//! (lines 1644-1792) and the customer-label family of §2.6 (lines 1551-1592).

mod common;

use axum::http::StatusCode;
use common::{spawn_app, TestApp};
use serde_json::json;

/// App with an admin, an agent whose primary team is `team_a`, and a second team.
struct Ctx {
    app: TestApp,
    admin_id: String,
    admin_token: String,
    agent_id: String,
    agent_token: String,
    team_a: i64,
    team_b: i64,
}

async fn setup() -> Ctx {
    let app = spawn_app().await;
    let admin_id = app.seed_agent("admin@test.com", "pw123456", "admin").await;
    let agent_id = app.seed_agent("agent@test.com", "pw123456", "agent").await;
    let team_a = app.seed_team("Team A").await;
    let team_b = app.seed_team("Team B").await;
    app.add_membership(&agent_id, team_a, "member", true).await;
    let (admin_token, _, _) = app.login("admin@test.com", "pw123456").await;
    let (agent_token, _, _) = app.login("agent@test.com", "pw123456").await;
    Ctx { app, admin_id, admin_token, agent_id, agent_token, team_a, team_b }
}

// -------------------------------------------------------------- list customers

#[tokio::test]
async fn list_customers_requires_auth() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/customers", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["success"], false);
}

#[tokio::test]
async fn list_customers_is_team_scoped() {
    let ctx = setup().await;
    let shared = ctx.app.seed_customer("line", "U-shared", "Shared", None).await;
    let mine = ctx.app.seed_customer("line", "U-mine", "Mine", Some(ctx.team_a)).await;
    ctx.app.seed_customer("line", "U-theirs", "Theirs", Some(ctx.team_b)).await;

    // Admin sees all three.
    let (status, body, _) =
        ctx.app.request("GET", "/api/customers", Some(&ctx.admin_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["count"], 3);
    assert_eq!(body["data"]["customers"].as_array().unwrap().len(), 3);
    assert!(body["timestamp"].is_string());

    // Agent sees own-team customers plus the shared (team-less) pool.
    let (status, body, _) =
        ctx.app.request("GET", "/api/customers", Some(&ctx.agent_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["count"], 2);
    let ids: Vec<i64> = body["data"]["customers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect();
    assert!(ids.contains(&shared) && ids.contains(&mine));

    // Customer record shape.
    let rec = &body["data"]["customers"][0];
    for key in ["id", "platform", "platform_user_id", "display_name", "avatar_url", "email",
                "phone", "source_team_id", "metadata", "created_at", "updated_at"] {
        assert!(rec.get(key).is_some(), "missing field {key}: {rec}");
    }
}

// ----------------------------------------------- get one customer + conversations

#[tokio::test]
async fn get_customer_returns_conversations() {
    let ctx = setup().await;
    let customer = ctx.app.seed_customer("line", "U1", "Cust", Some(ctx.team_a)).await;
    let conv = ctx.app.seed_conversation(customer, Some(ctx.team_a), "active").await;

    let (status, body, _) = ctx
        .app
        .request("GET", &format!("/api/customers/{customer}"), Some(&ctx.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["customer"]["id"], customer);
    assert_eq!(body["data"]["conversationCount"], 1);
    assert_eq!(body["data"]["conversations"][0]["id"], conv);
    assert_eq!(body["data"]["conversations"][0]["status"], "active");
}

#[tokio::test]
async fn get_customer_error_conditions() {
    let ctx = setup().await;
    let foreign = ctx.app.seed_customer("line", "U2", "Foreign", Some(ctx.team_b)).await;

    // Invalid id -> validation rejection before handler logic.
    let (status, _, _) = ctx
        .app
        .request("GET", "/api/customers/not-a-number", Some(&ctx.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Unknown id -> 404 "Customer not found".
    let (status, body, _) =
        ctx.app.request("GET", "/api/customers/99999", Some(&ctx.agent_token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Customer not found");

    // Out-of-scope -> indistinguishable 404 (never 403), hiding existence.
    let (status, body, _) = ctx
        .app
        .request("GET", &format!("/api/customers/{foreign}"), Some(&ctx.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Customer not found");

    // Admin may access any.
    let (status, _, _) = ctx
        .app
        .request("GET", &format!("/api/customers/{foreign}"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
}

// --------------------------------------------------- lookup by platform identity

#[tokio::test]
async fn get_customer_by_platform_identity() {
    let ctx = setup().await;
    let customer = ctx.app.seed_customer("line", "U-line-1", "P Cust", None).await;
    ctx.app.seed_conversation(customer, None, "active").await;

    let (status, body, _) = ctx
        .app
        .request("GET", "/api/customers/platform/line/U-line-1", Some(&ctx.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["customer"]["id"], customer);
    assert_eq!(body["data"]["conversationCount"], 1);

    // No match -> 404.
    let (status, body, _) = ctx
        .app
        .request("GET", "/api/customers/platform/line/unknown", Some(&ctx.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Customer not found");

    // Out-of-scope match -> same hidden 404.
    ctx.app.seed_customer("line", "U-b-1", "B Cust", Some(ctx.team_b)).await;
    let (status, body, _) = ctx
        .app
        .request("GET", "/api/customers/platform/line/U-b-1", Some(&ctx.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Customer not found");
}

// ------------------------------------------------------ selectable tag catalogue

#[tokio::test]
async fn available_tags_applies_team_scoping() {
    let ctx = setup().await;
    ctx.app.seed_tag_full("global", &ctx.admin_id, None, true).await;
    ctx.app.seed_tag_full("team-a", &ctx.admin_id, Some(ctx.team_a), true).await;
    ctx.app.seed_tag_full("team-b", &ctx.admin_id, Some(ctx.team_b), true).await;
    ctx.app.seed_tag_full("dormant", &ctx.admin_id, None, false).await;

    // Non-admin default: own team + global, active only, alphabetical.
    let (status, body, _) = ctx
        .app
        .request("GET", "/api/customers/tags/available", Some(&ctx.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true);
    let names: Vec<&str> =
        body["data"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["global", "team-a"]);
    assert_eq!(body["pagination"]["page"], 1);
    assert_eq!(body["pagination"]["limit"], 100);
    assert_eq!(body["pagination"]["total"], 2);
    assert_eq!(body["pagination"]["totalPages"], 1);
    assert!(body["message"].is_string());
    let tag = &body["data"][0];
    for key in ["id", "name", "color", "description", "teamId", "isActive", "createdBy",
                "createdAt", "updatedAt", "customerCount", "conversationCount"] {
        assert!(tag.get(key).is_some(), "missing field {key}: {tag}");
    }

    // Non-admin with includeGlobal=false: team tags only.
    let (_, body, _) = ctx
        .app
        .request(
            "GET",
            "/api/customers/tags/available?includeGlobal=false",
            Some(&ctx.agent_token),
            None,
        )
        .await;
    let names: Vec<&str> =
        body["data"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["team-a"]);

    // Admin default: all active tags.
    let (_, body, _) = ctx
        .app
        .request("GET", "/api/customers/tags/available", Some(&ctx.admin_token), None)
        .await;
    assert_eq!(body["pagination"]["total"], 3);

    // Admin with includeGlobal=false: team-scoped tags only.
    let (_, body, _) = ctx
        .app
        .request(
            "GET",
            "/api/customers/tags/available?includeGlobal=false",
            Some(&ctx.admin_token),
            None,
        )
        .await;
    assert_eq!(body["pagination"]["total"], 2);
}

#[tokio::test]
async fn available_tags_search_matches_wildcards_literally() {
    let ctx = setup().await;
    ctx.app.seed_tag_full("100%done", &ctx.admin_id, None, true).await;
    ctx.app.seed_tag_full("100xdone", &ctx.admin_id, None, true).await;

    let (status, body, _) = ctx
        .app
        .request(
            "GET",
            "/api/customers/tags/available?search=0%25d", // "0%d", URL-encoded
            Some(&ctx.admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> =
        body["data"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["100%done"], "% must not act as a wildcard: {body}");
}

// ------------------------------------------------------------ customer tag reads

#[tokio::test]
async fn get_customer_tags_returns_active_tags_only() {
    let ctx = setup().await;
    let customer = ctx.app.seed_customer("line", "U-tags", "Tagged", None).await;
    let active = ctx.app.seed_tag("active-tag", &ctx.admin_id).await;
    let inactive = ctx.app.seed_tag_full("inactive-tag", &ctx.admin_id, None, false).await;
    ctx.app.add_customer_tag(customer, active, &ctx.admin_id).await;
    ctx.app.add_customer_tag(customer, inactive, &ctx.admin_id).await;

    let (status, body, _) = ctx
        .app
        .request("GET", &format!("/api/customers/{customer}/tags"), Some(&ctx.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tags = body["data"].as_array().unwrap();
    assert_eq!(tags.len(), 1, "inactive tags must be omitted: {body}");
    assert_eq!(tags[0]["id"], active);
    assert_eq!(tags[0]["name"], "active-tag");
    assert!(tags[0]["teamId"].is_null());
    assert_eq!(tags[0]["assignedBy"], ctx.admin_id.as_str());
    assert!(tags[0]["assignedAt"].is_string());

    // Unknown customer -> 404.
    let (status, _, _) = ctx
        .app
        .request("GET", "/api/customers/99999/tags", Some(&ctx.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --------------------------------------------------------------- add tags

#[tokio::test]
async fn add_customer_tags_is_idempotent_per_tag() {
    let ctx = setup().await;
    let customer = ctx.app.seed_customer("line", "U-add", "AddMe", None).await;
    let t1 = ctx.app.seed_tag("add-1", &ctx.admin_id).await;
    let t2 = ctx.app.seed_tag("add-2", &ctx.admin_id).await;

    let (status, body, _) = ctx
        .app
        .request(
            "POST",
            &format!("/api/customers/{customer}/tags"),
            Some(&ctx.agent_token),
            Some(json!({ "tagIds": [t1, t2] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["added"], 2);
    assert_eq!(body["data"]["alreadyExists"], 0);
    assert!(body["message"].is_string());

    // Re-adding one plus a new duplicate-free check.
    let (status, body, _) = ctx
        .app
        .request(
            "POST",
            &format!("/api/customers/{customer}/tags"),
            Some(&ctx.agent_token),
            Some(json!({ "tagIds": [t1] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["added"], 0);
    assert_eq!(body["data"]["alreadyExists"], 1);

    // Reversible audit entries were recorded (one per added association).
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_logs WHERE action = 'tag assign' AND agent_id = ?",
    )
    .bind(&ctx.agent_id)
    .fetch_one(&ctx.app.state.db)
    .await
    .unwrap();
    assert_eq!(audits, 2);
}

#[tokio::test]
async fn add_customer_tags_error_conditions() {
    let ctx = setup().await;
    let customer = ctx.app.seed_customer("line", "U-add-err", "AddErr", None).await;
    let active = ctx.app.seed_tag("ok-tag", &ctx.admin_id).await;
    let inactive = ctx.app.seed_tag_full("off-tag", &ctx.admin_id, None, false).await;

    // Empty tagIds -> 422 on tagIds.
    let (status, body, _) = ctx
        .app
        .request(
            "POST",
            &format!("/api/customers/{customer}/tags"),
            Some(&ctx.agent_token),
            Some(json!({ "tagIds": [] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["data"]["errors"][0]["field"], "tagIds");

    // Unknown customer -> 404.
    let (status, _, _) = ctx
        .app
        .request(
            "POST",
            "/api/customers/99999/tags",
            Some(&ctx.agent_token),
            Some(json!({ "tagIds": [active] })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Inactive or unknown tags -> 422 "Some tag IDs are invalid or inactive".
    for bad in [inactive, 424242] {
        let (status, body, _) = ctx
            .app
            .request(
                "POST",
                &format!("/api/customers/{customer}/tags"),
                Some(&ctx.agent_token),
                Some(json!({ "tagIds": [active, bad] })),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
        assert_eq!(body["error"], "Some tag IDs are invalid or inactive");
    }
}

// -------------------------------------------------------------- remove tags

#[tokio::test]
async fn remove_customer_tags_detaches_and_is_noop_for_absent() {
    let ctx = setup().await;
    let customer = ctx.app.seed_customer("line", "U-rm", "RmMe", None).await;
    let t1 = ctx.app.seed_tag("rm-1", &ctx.admin_id).await;
    let t2 = ctx.app.seed_tag("rm-2", &ctx.admin_id).await;
    ctx.app.add_customer_tag(customer, t1, &ctx.admin_id).await;

    // t2 was never attached: harmless; message counts the requested list size.
    let (status, body, _) = ctx
        .app
        .request(
            "DELETE",
            &format!("/api/customers/{customer}/tags"),
            Some(&ctx.agent_token),
            Some(json!({ "tagIds": [t1, t2] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true);
    assert!(body["message"].as_str().unwrap().contains('2'));

    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM customer_tags WHERE customer_id = ?")
            .bind(customer)
            .fetch_one(&ctx.app.state.db)
            .await
            .unwrap();
    assert_eq!(remaining, 0);

    // One reversible unassign audit entry — only for the association that existed.
    let audits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM activity_logs WHERE action = 'tag unassign'")
            .fetch_one(&ctx.app.state.db)
            .await
            .unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn remove_customer_tags_error_conditions() {
    let ctx = setup().await;
    let customer = ctx.app.seed_customer("line", "U-rm-err", "RmErr", None).await;
    let tag = ctx.app.seed_tag("rm-err", &ctx.admin_id).await;

    let (status, body, _) = ctx
        .app
        .request(
            "DELETE",
            &format!("/api/customers/{customer}/tags"),
            Some(&ctx.agent_token),
            Some(json!({ "tagIds": [] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["data"]["errors"][0]["field"], "tagIds");

    let (status, _, _) = ctx
        .app
        .request(
            "DELETE",
            "/api/customers/99999/tags",
            Some(&ctx.agent_token),
            Some(json!({ "tagIds": [tag] })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------- replace tag set

#[tokio::test]
async fn replace_customer_tags_sets_exact_set_and_clears_with_empty() {
    let ctx = setup().await;
    let customer = ctx.app.seed_customer("line", "U-set", "SetMe", None).await;
    let t1 = ctx.app.seed_tag("set-1", &ctx.admin_id).await;
    let t2 = ctx.app.seed_tag("set-2", &ctx.admin_id).await;
    let t3 = ctx.app.seed_tag("set-3", &ctx.admin_id).await;
    ctx.app.add_customer_tag(customer, t1, &ctx.admin_id).await;

    // Wholesale replacement.
    let (status, body, _) = ctx
        .app
        .request(
            "PUT",
            &format!("/api/customers/{customer}/tags"),
            Some(&ctx.agent_token),
            Some(json!({ "tagIds": [t2, t3] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["totalTags"], 2);
    let attached: Vec<i64> = sqlx::query_scalar(
        "SELECT tag_id FROM customer_tags WHERE customer_id = ? ORDER BY tag_id",
    )
    .bind(customer)
    .fetch_all(&ctx.app.state.db)
    .await
    .unwrap();
    assert_eq!(attached, vec![t2, t3]);

    // Empty set clears all tags.
    let (status, body, _) = ctx
        .app
        .request(
            "PUT",
            &format!("/api/customers/{customer}/tags"),
            Some(&ctx.agent_token),
            Some(json!({ "tagIds": [] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["totalTags"], 0);
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM customer_tags WHERE customer_id = ?")
            .bind(customer)
            .fetch_one(&ctx.app.state.db)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn replace_customer_tags_error_conditions() {
    let ctx = setup().await;
    let customer = ctx.app.seed_customer("line", "U-set-err", "SetErr", None).await;
    let tag = ctx.app.seed_tag("set-err", &ctx.admin_id).await;
    let inactive = ctx.app.seed_tag_full("set-off", &ctx.admin_id, None, false).await;

    // tagIds not an array -> 422 "Tag IDs must be an array".
    for bad_body in [json!({}), json!({ "tagIds": "nope" })] {
        let (status, body, _) = ctx
            .app
            .request(
                "PUT",
                &format!("/api/customers/{customer}/tags"),
                Some(&ctx.agent_token),
                Some(bad_body),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"], "Tag IDs must be an array");
    }

    // Unknown customer -> 404.
    let (status, _, _) = ctx
        .app
        .request(
            "PUT",
            "/api/customers/99999/tags",
            Some(&ctx.agent_token),
            Some(json!({ "tagIds": [tag] })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Inactive tag in a non-empty set -> 422.
    let (status, body, _) = ctx
        .app
        .request(
            "PUT",
            &format!("/api/customers/{customer}/tags"),
            Some(&ctx.agent_token),
            Some(json!({ "tagIds": [inactive] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "Some tag IDs are invalid or inactive");
}
