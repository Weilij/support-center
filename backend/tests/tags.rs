//! Tags & Labeling behavior per Rust_CRD.md §2.6 (lines 1453-1644).

mod common;

use axum::http::StatusCode;
use common::{spawn_app, TestApp};
use serde_json::json;

async fn setup() -> (TestApp, String, String) {
    let app = spawn_app().await;
    let agent_id = app.seed_agent("agent@test.com", "pw123456", "agent").await;
    let (token, _, _) = app.login("agent@test.com", "pw123456").await;
    (app, agent_id, token)
}

async fn create_tag(app: &TestApp, token: &str, name: &str) -> i64 {
    let (status, body, _) = app
        .request("POST", "/api/tags", Some(token), Some(json!({ "name": name })))
        .await;
    assert_eq!(status, StatusCode::CREATED, "create tag failed: {body}");
    body["data"]["id"].as_i64().unwrap()
}

// ----------------------------------------------------------------- health probe

#[tokio::test]
async fn health_probe_is_public() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/tags/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["status"], "healthy");
    assert!(body["data"]["handler"].is_string());
    assert!(body["data"]["timestamp"].is_string());
    assert!(body["message"].is_string());
}

// ------------------------------------------------------------------ list labels

#[tokio::test]
async fn list_tags_requires_auth() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/tags", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["success"], false);
}

#[tokio::test]
async fn list_tags_returns_paginated_envelope_with_counts() {
    let (app, agent_id, token) = setup().await;
    let beta = create_tag(&app, &token, "beta").await;
    create_tag(&app, &token, "alpha").await;

    let customer = app.seed_customer("line", "U1", "Cust One", None).await;
    app.seed_conversation(customer, None, "active").await;
    app.add_customer_tag(customer, beta, &agent_id).await;

    let (status, body, _) = app.request("GET", "/api/tags", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let data = &body["data"];
    assert_eq!(data["page"], 1);
    assert_eq!(data["pageSize"], 50);
    assert_eq!(data["total"], 2);
    assert_eq!(data["totalPages"], 1);
    assert_eq!(data["hasNext"], false);
    assert_eq!(data["hasPrev"], false);
    let items = data["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    // Ordered alphabetically by name ascending.
    assert_eq!(items[0]["name"], "alpha");
    assert_eq!(items[1]["name"], "beta");
    // teamId/teamName/createdByName are always null in this listing.
    assert!(items[0]["teamId"].is_null());
    assert!(items[0]["teamName"].is_null());
    assert!(items[0]["createdByName"].is_null());
    assert_eq!(items[0]["isActive"], true);
    assert_eq!(items[0]["createdBy"], agent_id.as_str());
    assert_eq!(items[0]["customerCount"], 0);
    assert_eq!(items[1]["customerCount"], 1);
    assert_eq!(items[1]["conversationCount"], 1);
}

#[tokio::test]
async fn list_tags_supports_search_and_excludes_soft_deleted() {
    let (app, _agent_id, token) = setup().await;
    let doomed = create_tag(&app, &token, "vip customer").await;
    create_tag(&app, &token, "vip partner").await;
    create_tag(&app, &token, "spam").await;

    let (status, _, _) = app
        .request("DELETE", &format!("/api/tags/{doomed}"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body, _) = app.request("GET", "/api/tags?search=vip", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "soft-deleted tag must be excluded: {body}");
    assert_eq!(items[0]["name"], "vip partner");
}

// ----------------------------------------------------------------- create label

#[tokio::test]
async fn create_tag_returns_201_and_normalizes_color() {
    let (app, agent_id, token) = setup().await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/tags",
            Some(&token),
            Some(json!({ "name": "  urgent  ", "color": "#abc", "description": "hot", "teamId": 99 })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["success"], true);
    let data = &body["data"];
    assert_eq!(data["name"], "urgent"); // trimmed
    assert_eq!(data["color"], "#AABBCC"); // uppercase 6-digit normalization
    assert_eq!(data["description"], "hot");
    assert!(data["teamId"].is_null()); // teamId accepted but ignored
    assert_eq!(data["isActive"], true);
    assert_eq!(data["createdBy"], agent_id.as_str());
    assert_eq!(data["customerCount"], 0);
    assert_eq!(data["conversationCount"], 0);
    assert!(data["createdAt"].is_string());

    // Default color when omitted.
    let (_, body2, _) = app
        .request("POST", "/api/tags", Some(&token), Some(json!({ "name": "plain" })))
        .await;
    assert_eq!(body2["data"]["color"], "#3B82F6");
}

#[tokio::test]
async fn create_tag_requires_name() {
    let (app, _, token) = setup().await;
    let (status, body, _) = app
        .request("POST", "/api/tags", Some(&token), Some(json!({ "name": "   " })))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
    assert!(body["error"].as_str().unwrap().to_lowercase().contains("name"));
}

#[tokio::test]
async fn create_tag_rejects_invalid_color() {
    let (app, _, token) = setup().await;
    for bad in ["red", "#12345", "#GGGGGG", "3B82F6"] {
        let (status, body, _) = app
            .request("POST", "/api/tags", Some(&token), Some(json!({ "name": "x", "color": bad })))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "color {bad}: {body}");
        assert_eq!(body["data"]["code"], "VALIDATION_ERROR");
        assert_eq!(body["data"]["errors"][0]["field"], "color");
    }
}

#[tokio::test]
async fn create_tag_duplicate_active_name_conflicts() {
    let (app, _, token) = setup().await;
    create_tag(&app, &token, "vip").await;
    let (status, body, _) = app
        .request("POST", "/api/tags", Some(&token), Some(json!({ "name": "vip" })))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["success"], false);
}

#[tokio::test]
async fn create_tag_rejects_malformed_body_as_invalid_json() {
    let (app, _, token) = setup().await;
    // name as a number fails body deserialization -> 400 "Invalid JSON".
    let (status, body, _) = app
        .request("POST", "/api/tags", Some(&token), Some(json!({ "name": 123 })))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "Invalid JSON");
}

// ------------------------------------------------------------------ get single

#[tokio::test]
async fn get_tag_returns_detail_even_when_soft_deleted() {
    let (app, agent_id, token) = setup().await;
    let id = create_tag(&app, &token, "vip").await;

    let (status, body, _) = app.request("GET", &format!("/api/tags/{id}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let data = &body["data"];
    assert_eq!(data["id"], id);
    assert_eq!(data["name"], "vip");
    assert_eq!(data["isActive"], true);
    assert_eq!(data["createdBy"], agent_id.as_str());
    assert_eq!(data["createdByName"], "agent user");
    assert!(data["teamName"].is_null());
    assert_eq!(data["customerCount"], 0);
    assert_eq!(data["conversationCount"], 0);

    // Soft-delete, then the detail endpoint still returns the record.
    app.request("DELETE", &format!("/api/tags/{id}"), Some(&token), None).await;
    let (status, body, _) = app.request("GET", &format!("/api/tags/{id}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["isActive"], false);
}

#[tokio::test]
async fn get_tag_unknown_id_is_404() {
    let (app, _, token) = setup().await;
    let (status, body, _) = app.request("GET", "/api/tags/9999", Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["success"], false);
}

// ----------------------------------------------------------------- update label

#[tokio::test]
async fn update_tag_applies_changed_fields() {
    let (app, _, token) = setup().await;
    let id = create_tag(&app, &token, "old name").await;
    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/tags/{id}"),
            Some(&token),
            Some(json!({ "name": "new name", "color": "#0f0", "isActive": false })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["name"], "new name");
    assert_eq!(data["color"], "#00FF00");
    assert_eq!(data["isActive"], false);
    assert_eq!(data["customerCount"], 0);
    assert!(data["updatedAt"].is_string());
}

#[tokio::test]
async fn update_tag_with_no_effective_change_reports_no_change() {
    let (app, _, token) = setup().await;
    let id = create_tag(&app, &token, "stable").await;
    let (status, body, _) = app
        .request("PUT", &format!("/api/tags/{id}"), Some(&token), Some(json!({ "name": "stable" })))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "No changes made");
    assert_eq!(body["data"]["name"], "stable");
    assert_eq!(body["data"]["customerCount"], 0);
    assert_eq!(body["data"]["conversationCount"], 0);
}

#[tokio::test]
async fn update_tag_error_conditions() {
    let (app, _, token) = setup().await;
    let id = create_tag(&app, &token, "first").await;
    create_tag(&app, &token, "second").await;

    // Non-integer / non-positive id -> 400 "Invalid tag id".
    for bad in ["abc", "0", "-3"] {
        let (status, body, _) = app
            .request("PUT", &format!("/api/tags/{bad}"), Some(&token), Some(json!({ "name": "x" })))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(body["error"], "Invalid tag id");
    }

    // Missing tag -> 404.
    let (status, _, _) = app
        .request("PUT", "/api/tags/9999", Some(&token), Some(json!({ "name": "x" })))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Invalid color -> 422 on `color`.
    let (status, body, _) = app
        .request("PUT", &format!("/api/tags/{id}"), Some(&token), Some(json!({ "color": "blue" })))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["data"]["errors"][0]["field"], "color");

    // Duplicate name -> 422 on `name`.
    let (status, body, _) = app
        .request("PUT", &format!("/api/tags/{id}"), Some(&token), Some(json!({ "name": "second" })))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["data"]["errors"][0]["field"], "name");
}

// ------------------------------------------------------------ soft-delete label

#[tokio::test]
async fn delete_tag_soft_deletes_and_repeated_delete_is_404() {
    let (app, _, token) = setup().await;
    let id = create_tag(&app, &token, "ephemeral").await;

    let (status, body, _) =
        app.request("DELETE", &format!("/api/tags/{id}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert!(body["data"].is_null());
    assert!(body["message"].is_string());

    // Already soft-deleted -> 404.
    let (status, _, _) = app.request("DELETE", &format!("/api/tags/{id}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Invalid id -> 400 "Invalid tag id".
    let (status, body, _) = app.request("DELETE", "/api/tags/zero", Some(&token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "Invalid tag id");
}

// -------------------------------------------------------------- bulk operations

#[tokio::test]
async fn bulk_operations_apply_to_all_listed_tags() {
    let (app, _, token) = setup().await;
    let a = create_tag(&app, &token, "bulk-a").await;
    let b = create_tag(&app, &token, "bulk-b").await;

    // Deactivate (numeric and digit-string identifiers both accepted).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/tags/bulk",
            Some(&token),
            Some(json!({ "operation": "deactivate", "tagIds": [a, b.to_string()] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"].is_null());
    assert!(body["message"].as_str().unwrap().contains("deactivate"));
    let (_, one, _) = app.request("GET", &format!("/api/tags/{a}"), Some(&token), None).await;
    assert_eq!(one["data"]["isActive"], false);

    // Activate back.
    app.request(
        "POST",
        "/api/tags/bulk",
        Some(&token),
        Some(json!({ "operation": "activate", "tagIds": [a, b] })),
    )
    .await;
    let (_, one, _) = app.request("GET", &format!("/api/tags/{a}"), Some(&token), None).await;
    assert_eq!(one["data"]["isActive"], true);

    // update_color stores the value as supplied without HEX validation.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/tags/bulk",
            Some(&token),
            Some(json!({ "operation": "update_color", "tagIds": [a], "data": { "color": "not-a-hex" } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, one, _) = app.request("GET", &format!("/api/tags/{a}"), Some(&token), None).await;
    assert_eq!(one["data"]["color"], "not-a-hex");
}

#[tokio::test]
async fn bulk_operation_error_conditions() {
    let (app, _, token) = setup().await;
    let a = create_tag(&app, &token, "bulk-err").await;

    // Empty tagIds -> 422 on tagIds.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/tags/bulk",
            Some(&token),
            Some(json!({ "operation": "activate", "tagIds": [] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["data"]["errors"][0]["field"], "tagIds");

    // Non-numeric identifier -> 400.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/tags/bulk",
            Some(&token),
            Some(json!({ "operation": "activate", "tagIds": ["12a"] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "Invalid tag ID format detected");

    // update_color without a color -> 422 on data.color.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/tags/bulk",
            Some(&token),
            Some(json!({ "operation": "update_color", "tagIds": [a], "data": {} })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["data"]["errors"][0]["field"], "data.color");

    // Unknown operation -> 422 on operation.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/tags/bulk",
            Some(&token),
            Some(json!({ "operation": "explode", "tagIds": [a] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["data"]["errors"][0]["field"], "operation");
}

// ------------------------------------------------------------- usage statistics

#[tokio::test]
async fn tag_stats_aggregates_usage() {
    let (app, agent_id, token) = setup().await;
    let tag = create_tag(&app, &token, "stats").await;
    let line_customer = app.seed_customer("line", "U1", "Line Cust", None).await;
    let fb_customer = app.seed_customer("facebook", "F1", "FB Cust", None).await;
    app.seed_conversation(line_customer, None, "active").await;
    app.seed_conversation(line_customer, None, "closed").await;
    app.add_customer_tag(line_customer, tag, &agent_id).await;
    app.add_customer_tag(fb_customer, tag, &agent_id).await;

    let (status, body, _) =
        app.request("GET", &format!("/api/tags/{tag}/stats"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["tagInfo"]["id"], tag);
    assert_eq!(data["tagInfo"]["name"], "stats");
    assert_eq!(data["customers"]["total"], 2);
    assert_eq!(data["customers"]["byPlatform"]["line"], 1);
    assert_eq!(data["customers"]["byPlatform"]["facebook"], 1);
    assert_eq!(data["conversations"]["total"], 2);
    assert_eq!(data["conversations"]["active"], 1);
    assert_eq!(data["conversations"]["closed"], 1);
    let trend = data["usageTrend"].as_array().unwrap();
    assert_eq!(trend.len(), 1);
    assert_eq!(trend[0]["assignments"], 2);
    let assigners = data["topAssigners"].as_array().unwrap();
    assert_eq!(assigners.len(), 1);
    assert_eq!(assigners[0]["name"], "agent user");
    assert_eq!(assigners[0]["assignments"], 2);
}

#[tokio::test]
async fn tag_stats_unknown_id_is_404() {
    let (app, _, token) = setup().await;
    let (status, _, _) = app.request("GET", "/api/tags/4242/stats", Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------- label customers

#[tokio::test]
async fn tag_customers_lists_holders_with_pagination() {
    let (app, agent_id, token) = setup().await;
    let tag = create_tag(&app, &token, "holders").await;
    let customer = app.seed_customer("line", "U9", "Holder", None).await;
    app.add_customer_tag(customer, tag, &agent_id).await;

    let (status, body, _) =
        app.request("GET", &format!("/api/tags/{tag}/customers"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let customers = body["data"]["customers"].as_array().unwrap();
    assert_eq!(customers.len(), 1);
    assert_eq!(customers[0]["id"], customer);
    assert_eq!(customers[0]["platform"], "line");
    assert_eq!(customers[0]["platform_user_id"], "U9");
    assert_eq!(customers[0]["display_name"], "Holder");
    assert_eq!(customers[0]["assigned_by"], agent_id.as_str());
    assert!(customers[0]["assigned_at"].is_string());
    let p = &body["data"]["pagination"];
    assert_eq!(p["page"], 1);
    assert_eq!(p["limit"], 50);
    assert_eq!(p["total"], 1);
    assert_eq!(p["totalPages"], 1);
}

#[tokio::test]
async fn tag_customers_requires_active_tag() {
    let (app, agent_id, token) = setup().await;
    let inactive = app.seed_tag_full("inactive", &agent_id, None, false).await;
    let (status, _, _) =
        app.request("GET", &format!("/api/tags/{inactive}/customers"), Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------- label conversations

#[tokio::test]
async fn tag_conversations_lists_holder_conversations() {
    let (app, agent_id, token) = setup().await;
    let tag = create_tag(&app, &token, "convs").await;
    let customer = app.seed_customer("line", "U7", "Conv Holder", None).await;
    let conv = app.seed_conversation(customer, None, "active").await;
    app.add_customer_tag(customer, tag, &agent_id).await;

    let (status, body, _) =
        app.request("GET", &format!("/api/tags/{tag}/conversations"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let convs = body["data"]["conversations"].as_array().unwrap();
    assert_eq!(convs.len(), 1);
    assert_eq!(convs[0]["id"], conv);
    assert_eq!(convs[0]["status"], "active");
    assert_eq!(convs[0]["channel"], "line");
    assert_eq!(convs[0]["customer_name"], "Conv Holder");
    assert_eq!(convs[0]["customer_platform"], "line");
    assert_eq!(convs[0]["assigned_by"], agent_id.as_str());
    let p = &body["data"]["pagination"];
    assert_eq!(p["limit"], 20);
    assert_eq!(p["total"], 1);

    // Unknown tag -> 404.
    let (status, _, _) =
        app.request("GET", "/api/tags/31337/conversations", Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------ conversation-label family

#[tokio::test]
async fn conversation_tags_add_list_remove() {
    let (app, agent_id, token) = setup().await;
    let tag_a = create_tag(&app, &token, "conv-a").await;
    let tag_b = create_tag(&app, &token, "conv-b").await;
    let customer = app.seed_customer("line", "U5", "C", None).await;
    let conv = app.seed_conversation(customer, None, "active").await;

    // Add (duplicates suppressed on re-add).
    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/conversations/{conv}/tags"),
            Some(&token),
            Some(json!({ "tagIds": [tag_a, tag_b] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true);
    assert!(body["message"].is_string());
    app.request(
        "POST",
        &format!("/api/conversations/{conv}/tags"),
        Some(&token),
        Some(json!({ "tagIds": [tag_a] })),
    )
    .await;

    // List.
    let (status, body, _) = app
        .request("GET", &format!("/api/conversations/{conv}/tags"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let tags = body["data"].as_array().unwrap();
    assert_eq!(tags.len(), 2, "duplicate add must be suppressed: {body}");
    assert_eq!(tags[0]["assignedBy"], agent_id.as_str());
    assert!(tags[0]["assignedAt"].is_string());

    // Remove one.
    let (status, body, _) = app
        .request(
            "DELETE",
            &format!("/api/conversations/{conv}/tags"),
            Some(&token),
            Some(json!({ "tagIds": [tag_a] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body, _) = app
        .request("GET", &format!("/api/conversations/{conv}/tags"), Some(&token), None)
        .await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn conversation_tags_error_conditions() {
    let (app, _, token) = setup().await;
    let tag = create_tag(&app, &token, "conv-err").await;

    // Unknown conversation -> 404 "Conversation not found".
    let (status, body, _) =
        app.request("GET", "/api/conversations/nope/tags", Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Conversation not found");
    let (status, _, _) = app
        .request(
            "POST",
            "/api/conversations/nope/tags",
            Some(&token),
            Some(json!({ "tagIds": [tag] })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Empty tagIds -> 422 on tagIds (validated before conversation lookup).
    let customer = app.seed_customer("line", "U6", "C", None).await;
    let conv = app.seed_conversation(customer, None, "active").await;
    for method in ["POST", "DELETE"] {
        let (status, body, _) = app
            .request(
                method,
                &format!("/api/conversations/{conv}/tags"),
                Some(&token),
                Some(json!({ "tagIds": [] })),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{method}");
        assert_eq!(body["data"]["errors"][0]["field"], "tagIds");
    }
}
