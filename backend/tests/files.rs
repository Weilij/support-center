//! File & Attachment Management per CRD §4.4 (lines 2996-3216).

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{spawn_app, TestApp};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3, 4];

async fn agent(app: &TestApp) -> String {
    app.seed_agent("files@test.dev", "pw123456", "agent").await;
    app.login("files@test.dev", "pw123456").await.0
}

/// Multipart upload helper.
async fn upload(
    app: &TestApp,
    token: &str,
    path: &str,
    filename: &str,
    content_type: &str,
    bytes: &[u8],
    extra_fields: &[(&str, &str)],
) -> (StatusCode, Value) {
    let boundary = "X-TEST-BOUNDARY";
    let mut body: Vec<u8> = Vec::new();
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend(bytes);
    body.extend(b"\r\n");
    for (k, v) in extra_fields {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                .as_bytes(),
        );
    }
    body.extend(format!("--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn get_raw(app: &TestApp, path: &str) -> (StatusCode, Vec<u8>) {
    let req = Request::builder().uri(path).body(Body::empty()).unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, bytes)
}

// ---------------------------------------------------------------- basics

#[tokio::test]
async fn health_is_public_and_info_requires_auth() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/files/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["module"], "files");
    assert_eq!(body["data"]["databaseAvailable"], true);
    assert_eq!(body["data"]["storageAvailable"], true);

    let (status, _, _) = app.request("GET", "/api/files/info", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let token = agent(&app).await;
    let (status, body, _) = app.request("GET", "/api/files/info", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["limits"]["maxFileSize"], "10MB");
}

#[tokio::test]
async fn upload_validates_and_persists() {
    let app = spawn_app().await;
    let token = agent(&app).await;

    // Valid PNG upload.
    let (status, body) =
        upload(&app, &token, "/api/files", "pic.png", "image/png", PNG, &[]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let data = &body["data"];
    assert_eq!(data["fileType"], "image");
    assert_eq!(data["size"], PNG.len());
    assert!(data["url"].as_str().unwrap().contains("sig="));
    assert!(data["publicUrl"].as_str().unwrap().contains("/api/files/public/"));
    assert!(data["thumbnailUrl"].is_string(), "image gets a thumbnail reference");

    // Wrong magic bytes for the declared type -> corrupted.
    let (status, body) =
        upload(&app, &token, "/api/files", "fake.png", "image/png", b"not-a-png", &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("corrupted"), "{body}");

    // Blocked extension.
    let (status, _) =
        upload(&app, &token, "/api/files", "evil.exe", "image/png", PNG, &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Disallowed content type.
    let (status, _) = upload(
        &app, &token, "/api/files", "app.wasm", "application/wasm", b"\0asm", &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Empty file.
    let (status, _) = upload(&app, &token, "/api/files", "empty.png", "image/png", b"", &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Platform subset: LINE forbids documents.
    let (status, _) = upload(
        &app, &token, "/api/files", "doc.pdf", "application/pdf", b"%PDF-1.4 x",
        &[("platform", "line")],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_is_scoped_to_uploader_for_non_admins() {
    let app = spawn_app().await;
    let token = agent(&app).await;
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    let (admin, _, _) = app.login("admin@test.dev", "pw123456").await;
    app.seed_agent("other@test.dev", "pw123456", "agent").await;
    let (other, _, _) = app.login("other@test.dev", "pw123456").await;

    upload(&app, &token, "/api/files", "mine.png", "image/png", PNG, &[]).await;
    upload(&app, &other, "/api/files", "theirs.png", "image/png", PNG, &[]).await;

    let (_, body, _) = app.request("GET", "/api/files", Some(&token), None).await;
    assert_eq!(body["data"]["total"], 1, "non-admin sees only own uploads");
    assert_eq!(body["data"]["items"][0]["filename"], "mine.png");

    let (_, body, _) = app.request("GET", "/api/files", Some(&admin), None).await;
    assert_eq!(body["data"]["total"], 2, "admin sees all");
}

#[tokio::test]
async fn per_file_modes_delete_and_id_validation() {
    let app = spawn_app().await;
    let token = agent(&app).await;
    let (_, body) = upload(&app, &token, "/api/files", "x.png", "image/png", PNG, &[]).await;
    let id = body["data"]["id"].as_str().unwrap().to_string();

    // Bad identifier format.
    let (status, _, _) = app.request("GET", "/api/files/bad%2Fid", Some(&token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // url mode returns a fresh signed URL.
    let (status, body, _) = app
        .request("GET", &format!("/api/files/{id}?mode=url"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["url"].as_str().unwrap().contains("sig="));

    // Stream mode returns raw bytes with attachment disposition.
    let req = Request::builder()
        .uri(format!("/api/files/{id}"))
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("attachment"));
    let raw = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(raw.as_ref(), PNG);

    // Hard delete is idempotent against a missing object and removes the record.
    let (status, _, _) = app.request("DELETE", &format!("/api/files/{id}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = app.request("GET", &format!("/api/files/{id}?mode=url"), Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = app.request("DELETE", &format!("/api/files/{id}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------- signed proxies

#[tokio::test]
async fn public_proxy_requires_a_valid_unexpired_signature() {
    let app = spawn_app().await;
    let token = agent(&app).await;
    let (_, body) = upload(&app, &token, "/api/files", "p.png", "image/png", PNG, &[]).await;
    let public_url = body["data"]["publicUrl"].as_str().unwrap().to_string();
    let path = public_url.trim_start_matches(
        app.state.config.backend_url.as_deref().unwrap_or(""),
    );

    // Valid signature streams the object with CORS + cache headers.
    let (status, bytes) = get_raw(&app, path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, PNG);

    // Missing/forged signature -> 404 (never 401, CRD 3119).
    let bare = path.split('?').next().unwrap();
    let (status, _) = get_raw(&app, bare).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get_raw(&app, &format!("{bare}?expires=99999999999&sig=deadbeef")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Expired signature -> 404.
    let key = bare.trim_start_matches("/api/files/public/");
    let (sig, _) = mcss_backend::domain::files::sign::sign(
        &app.state.config.jwt_secret, key, -10,
    );
    let (status, _) = get_raw(&app, &format!("{bare}?expires={}&sig={sig}",
        chrono::Utc::now().timestamp() - 10)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn public_download_forces_attachment_and_fixes_extension() {
    let app = spawn_app().await;
    let token = agent(&app).await;
    let (_, body) = upload(&app, &token, "/api/files", "report.png", "image/png", PNG, &[]).await;
    let id = body["data"]["id"].as_str().unwrap().to_string();

    // Strip the stored filename's extension to exercise the append rule.
    sqlx::query("UPDATE attachments SET file_name = 'report' WHERE id = ?")
        .bind(&id)
        .execute(&app.state.db)
        .await
        .unwrap();

    let (_, body, _) = app
        .request("GET", &format!("/api/files/{id}/download-url"), Some(&token), None)
        .await;
    let url = body["data"]["url"].as_str().unwrap().to_string();
    let path = url.trim_start_matches(app.state.config.backend_url.as_deref().unwrap_or(""));

    let req = Request::builder().uri(path).body(Body::empty()).unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let disposition = resp.headers().get("content-disposition").unwrap().to_str().unwrap();
    assert!(disposition.contains("report.png"), "extension appended: {disposition}");

    // Unknown attachment id -> 404.
    let (status, _) = get_raw(&app, "/api/files/download/ghost?expires=1&sig=aa").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn r2_public_proxy_serves_signed_objects_with_long_cache() {
    let app = spawn_app().await;
    mcss_backend::domain::files::store::put_object(
        &app.state.config.upload_dir, "qr/team-1.svg", b"<svg/>",
    )
    .await
    .unwrap();
    let (sig, expires) =
        mcss_backend::domain::files::sign::sign(&app.state.config.jwt_secret, "qr/team-1.svg", 600);

    let req = Request::builder()
        .uri(format!("/api/r2-public/qr/team-1.svg?expires={expires}&sig={sig}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("etag").is_some());
    assert!(resp
        .headers()
        .get("cache-control")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("31536000"));

    let (status, _) = get_raw(&app, "/api/r2-public/qr/team-1.svg?expires=1&sig=bad").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn line_proxy_serves_stored_media_and_validates_id() {
    let app = spawn_app().await;
    let (status, _) = get_raw(&app, "/api/files/line-proxy/not-digits").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Fast path: stored copy exists.
    mcss_backend::domain::files::store::put_object(
        &app.state.config.upload_dir, "line/media/12345", PNG,
    )
    .await
    .unwrap();
    let (status, bytes) = get_raw(&app, "/api/files/line-proxy/12345").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, PNG);

    // No stored copy: stubbed upstream reports bad-gateway (CRD 3138).
    let (status, _) = get_raw(&app, "/api/files/line-proxy/99999").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

// ---------------------------------------------------------------- direct upload

#[tokio::test]
async fn direct_upload_flow_pending_confirm_completed() {
    let app = spawn_app().await;
    let token = agent(&app).await;

    // Validation.
    let (status, _, _) = app
        .request("POST", "/api/files/presigned-url", Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _, _) = app
        .request("POST", "/api/files/presigned-url", Some(&token),
            Some(json!({"filename": "a.bin", "contentType": "application/x-msdownload", "size": 10})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("POST", "/api/files/presigned-url", Some(&token),
            Some(json!({"filename": "a.png", "contentType": "image/png", "size": 99999999})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Mint the target.
    let (status, body, _) = app
        .request("POST", "/api/files/presigned-url", Some(&token),
            Some(json!({"filename": "direct.png", "contentType": "image/png", "size": PNG.len()})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let file_id = body["data"]["fileId"].as_str().unwrap().to_string();
    let upload_url = body["data"]["uploadUrl"].as_str().unwrap().to_string();
    assert_eq!(body["data"]["instructions"]["method"], "PUT");

    // Pending status before upload.
    let (_, sbody, _) = app
        .request("GET", &format!("/api/files/{file_id}/status"), Some(&token), None)
        .await;
    assert_eq!(sbody["data"]["uploadStatus"], "pending");

    // Confirm before object exists -> failed.
    let (status, _, _) = app
        .request("POST", &format!("/api/files/{file_id}/confirm"), Some(&token),
            Some(json!({"size": PNG.len()})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, sbody, _) = app
        .request("GET", &format!("/api/files/{file_id}/status"), Some(&token), None)
        .await;
    assert_eq!(sbody["data"]["uploadStatus"], "failed");

    // New target; PUT the bytes to the signed URL; confirm -> completed.
    let (_, body, _) = app
        .request("POST", "/api/files/presigned-url", Some(&token),
            Some(json!({"filename": "direct2.png", "contentType": "image/png", "size": PNG.len()})))
        .await;
    let file_id = body["data"]["fileId"].as_str().unwrap().to_string();
    let upload_path = body["data"]["uploadUrl"]
        .as_str()
        .unwrap()
        .trim_start_matches(app.state.config.backend_url.as_deref().unwrap_or(""))
        .to_string();
    let req = Request::builder()
        .method("PUT")
        .uri(&upload_path)
        .body(Body::from(PNG.to_vec()))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "signed direct PUT accepted");

    let (status, cbody, _) = app
        .request("POST", &format!("/api/files/{file_id}/confirm"), Some(&token),
            Some(json!({"size": PNG.len()})))
        .await;
    assert_eq!(status, StatusCode::OK, "{cbody}");
    assert_eq!(cbody["data"]["confirmed"], true);
    assert_eq!(cbody["data"]["uploadStatus"], "completed");

    // Idempotent re-confirm.
    let (status, cbody, _) = app
        .request("POST", &format!("/api/files/{file_id}/confirm"), Some(&token),
            Some(json!({"size": PNG.len()})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cbody["data"]["confirmed"], true);

    // Subsystem status.
    let (_, body, _) = app
        .request("GET", "/api/files/presigned-url/status", Some(&token), None)
        .await;
    assert_eq!(body["data"]["configured"], true);
    assert_eq!(body["data"]["maxMB"], 10);
    let _ = upload_url;
}

// ---------------------------------------------------------------- richer ops

#[tokio::test]
async fn multi_upload_allows_partial_success() {
    let app = spawn_app().await;
    let token = agent(&app).await;

    // Two files: one valid PNG, one corrupted.
    let boundary = "X-MULTI";
    let mut body: Vec<u8> = Vec::new();
    for (name, bytes) in [("ok.png", PNG), ("bad.png", b"junk".as_slice())] {
        body.extend(format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{name}\"\r\nContent-Type: image/png\r\n\r\n"
        ).as_bytes());
        body.extend(bytes);
        body.extend(b"\r\n");
    }
    body.extend(format!("--{boundary}--\r\n").as_bytes());
    let req = Request::builder()
        .method("POST")
        .uri("/api/files/upload-multiple")
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let parsed: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(parsed["data"]["summary"]["total"], 2);
    assert_eq!(parsed["data"]["summary"]["successCount"], 1);
    assert_eq!(parsed["data"]["summary"]["failedCount"], 1);
    assert_eq!(parsed["data"]["failed"][0]["filename"], "bad.png");
}

#[tokio::test]
async fn search_and_batch_delete() {
    let app = spawn_app().await;
    let token = agent(&app).await;
    let (_, a) = upload(&app, &token, "/api/files", "invoice-march.png", "image/png", PNG, &[]).await;
    let (_, b) = upload(&app, &token, "/api/files", "photo.png", "image/png", PNG, &[]).await;
    let id_a = a["data"]["id"].as_str().unwrap().to_string();
    let id_b = b["data"]["id"].as_str().unwrap().to_string();

    let (status, _, _) = app.request("GET", "/api/files/search", Some(&token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "query required");

    let (_, body, _) = app.request("GET", "/api/files/search?q=INVOICE", Some(&token), None).await;
    assert_eq!(body["data"]["total"], 1, "case-insensitive contains");

    // Batch delete with one unknown id and an unsupported op per item.
    let (status, body, _) = app
        .request("POST", "/api/files/batch", Some(&token),
            Some(json!({"operation": "delete", "fileIds": [id_a, "ghost"]})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["summary"]["successCount"], 1);
    assert_eq!(body["data"]["summary"]["failedCount"], 1);
    assert!(body["data"]["summary"]["processingTimeMs"].is_i64());

    let (status, body, _) = app
        .request("POST", "/api/files/batch", Some(&token),
            Some(json!({"operation": "shred", "fileIds": [id_b]})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["summary"]["failedCount"], 1, "unknown op fails per-item");

    let (status, _, _) = app
        .request("POST", "/api/files/batch", Some(&token),
            Some(json!({"operation": "delete", "fileIds": []})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn stats_and_chunked_lifecycle() {
    let app = spawn_app().await;
    let token = agent(&app).await;
    upload(&app, &token, "/api/files", "s.png", "image/png", PNG, &[]).await;

    let (status, body, _) = app
        .request("GET", "/api/files/stats/summary", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["totalFiles"], 1);
    assert_eq!(body["data"]["byType"][0]["type"], "image");
    assert_eq!(body["data"]["recentActivity"]["uploads"], 1);

    // Chunked: init computes chunk plan; the rest are acknowledgements.
    let (status, body, _) = app
        .request("POST", "/api/files/chunked/init", Some(&token),
            Some(json!({"filename": "big.zip", "size": 2_500_000, "contentType": "application/zip"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["chunkSize"], 1024 * 1024);
    assert_eq!(body["data"]["totalChunks"], 3);
    let session = body["data"]["uploadId"].as_str().unwrap().to_string();
    for step in ["chunk", "complete"] {
        let (status, _, _) = app
            .request("POST", &format!("/api/files/chunked/{session}/{step}"), Some(&token),
                Some(json!({})))
            .await;
        assert_eq!(status, StatusCode::OK, "{step}");
    }
}

#[tokio::test]
async fn conversation_and_message_scoped_listings() {
    let app = spawn_app().await;
    let token = agent(&app).await;
    let customer = app.seed_customer("line", "U-f", "F", None).await;
    let conversation = app.seed_conversation(customer, None, "active").await;
    upload(&app, &token, "/api/files", "c.png", "image/png", PNG,
        &[("conversationId", conversation.as_str())]).await;

    let (status, body, _) = app
        .request("GET", &format!("/api/files/conversation/{conversation}"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], 1);

    let (status, body, _) = app
        .request("GET", "/api/files/message/some-message", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}
