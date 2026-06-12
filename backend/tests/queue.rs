//! Background Queue Processing per CRD §6.5 (lines 5106-5245).

mod common;

use axum::http::StatusCode;
use common::{spawn_app, TestApp};
use mcss_backend::domain::queue::worker;
use serde_json::json;

const PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];

async fn agent(app: &TestApp) -> String {
    app.seed_agent("q@test.dev", "pw123456", "agent").await;
    app.login("q@test.dev", "pw123456").await.0
}

/// Poll until `check` returns true or the timeout elapses.
async fn wait_for<F, Fut>(timeout_ms: u64, mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        if check().await {
            return true;
        }
        if tokio::time::Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

async fn seed_pending_message(app: &TestApp) -> (String, String) {
    let customer = app.seed_customer("line", "U-q", "Q", None).await;
    let conversation = app.seed_conversation(customer, None, "active").await;
    let message_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO messages (id, conversation_id, sender_type, content, content_type,
                               delivery_status, created_at)
         VALUES (?, ?, 'agent', 'queued hello', 'text', 'pending', ?)",
    )
    .bind(&message_id)
    .bind(&conversation)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();
    (conversation, message_id)
}

// ---------------------------------------------------------------- monitoring

#[tokio::test]
async fn monitoring_endpoints_require_auth_and_report_shapes() {
    let app = spawn_app().await;
    for path in ["/api/queues/stats", "/api/queues/health", "/api/queues/performance"] {
        let (status, _, _) = app.request("GET", path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
    let token = agent(&app).await;

    let (status, body, _) = app.request("GET", "/api/queues/stats", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["summary"]["status"], "healthy");
    let mq = &body["data"]["queues"]["messageQueue"];
    assert_eq!(mq["configuration"]["maxBatchSize"], 10);
    assert_eq!(mq["configuration"]["maxBatchTimeout"], 5);
    assert_eq!(mq["configuration"]["retryPolicy"], "exponential-backoff");
    assert!(body["data"]["systemHealth"]["lastCheck"].is_string());

    let (_, body, _) = app.request("GET", "/api/queues/health", Some(&token), None).await;
    assert_eq!(body["data"]["queues"]["available"], true);
    assert_eq!(body["data"]["queues"]["processingLatency"], "<100ms");

    let (_, body, _) = app.request("GET", "/api/queues/performance", Some(&token), None).await;
    assert!(body["data"]["throughput"]["messagesPerSecond"].is_number());
    assert!(body["data"]["reliability"]["successRate"].is_number());
}

#[tokio::test]
async fn maintenance_dispatches_and_lists_available_operations() {
    let app = spawn_app().await;
    let token = agent(&app).await;
    let (status, body, _) = app
        .request("POST", "/api/queues/maintenance", Some(&token),
            Some(json!({"operation": "status"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["queueStatus"], "healthy");

    let (status, body, _) = app
        .request("POST", "/api/queues/maintenance", Some(&token),
            Some(json!({"operation": "defragment"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["availableOperations"].is_array());
}

// ---------------------------------------------------------------- outbound jobs

#[tokio::test]
async fn outbound_job_delivers_and_marks_message() {
    let app = spawn_app().await;
    worker::spawn(app.state.clone());
    let (conversation, message_id) = seed_pending_message(&app).await;

    let ack = app.state.queue.enqueue_outbound(json!({
        "messageId": message_id,
        "conversationId": conversation,
        "recipientId": "U-q",
        "content": "queued hello",
        "messageType": "text",
        "metadata": { "agentId": "a-1", "enqueuedAt": 0, "retryCount": 0 },
    }));
    assert_eq!(ack["success"], true, "fire-and-forget acceptance");
    assert!(ack.get("error").is_none());

    let delivered = wait_for(5000, || async {
        sqlx::query_scalar::<_, String>("SELECT delivery_status FROM messages WHERE id = ?")
            .bind(&message_id)
            .fetch_one(&app.state.db)
            .await
            .map(|s| s == "delivered")
            .unwrap_or(false)
    })
    .await;
    assert!(delivered, "message transitioned to delivered");

    let stats = app.state.queue.stats.lock().unwrap().clone();
    assert!(stats.successes >= 1);
    assert!(stats.last_processed_at.is_some());
}

#[tokio::test]
async fn placeholder_only_content_fails_without_retry() {
    let app = spawn_app().await;
    worker::spawn(app.state.clone());
    let (conversation, message_id) = seed_pending_message(&app).await;

    // A bracketed placeholder with no attachments assembles an empty set
    // (CRD 5165-5166): validation failures are never retried (CRD 5189).
    app.state.queue.enqueue_outbound(json!({
        "messageId": message_id,
        "conversationId": conversation,
        "recipientId": "U-q",
        "content": "[圖片]",
        "metadata": { "agentId": "a-1", "retryCount": 0 },
    }));

    let dead = wait_for(5000, || async { app.state.queue.dead_letter_size() == 1 }).await;
    assert!(dead, "non-retryable job dead-lettered immediately");
    let status: String = sqlx::query_scalar("SELECT delivery_status FROM messages WHERE id = ?")
        .bind(&message_id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(status, "failed");
    let stats = app.state.queue.stats.lock().unwrap().clone();
    assert_eq!(stats.retries, 0, "validation errors skip the retry path");
}

// ---------------------------------------------------------------- media jobs

#[tokio::test]
async fn media_job_stores_attachment_idempotently() {
    let app = spawn_app().await;
    worker::spawn(app.state.clone());
    let (conversation, message_id) = seed_pending_message(&app).await;

    // The platform content is already mirrored in the store (stubbed fetch).
    mcss_backend::domain::files::store::put_object(
        &app.state.config.upload_dir, "line/media/777", PNG,
    )
    .await
    .unwrap();

    let job = json!({
        "type": "media_processing",
        "messageId": message_id,
        "conversationId": conversation,
        "platformMessageId": "777",
        "mediaType": "image",
        "enqueuedAt": 0,
    });
    app.state.queue.enqueue_media(job.clone());
    let stored = wait_for(5000, || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM attachments WHERE message_id = ?")
            .bind(&message_id)
            .fetch_one(&app.state.db)
            .await
            .map(|c| c == 1)
            .unwrap_or(false)
    })
    .await;
    assert!(stored, "attachment record created");

    // Re-delivery of the same job does not duplicate the record.
    app.state.queue.enqueue_media(job);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE message_id = ?")
            .bind(&message_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(count, 1, "duplicate attachment records are avoided");
}

#[tokio::test]
async fn failed_media_download_retries_then_dead_letters() {
    let app = spawn_app().await;
    worker::spawn(app.state.clone());
    let (conversation, message_id) = seed_pending_message(&app).await;

    // No stored object: the stubbed download fails as a network-class error,
    // which is retryable up to 3 attempts (CRD 5161, 5189).
    app.state.queue.enqueue_media(json!({
        "type": "media_processing",
        "messageId": message_id,
        "conversationId": conversation,
        "platformMessageId": "404404",
        "mediaType": "image",
    }));

    // Backoff 1s + 2s before the third (final) attempt.
    let dead = wait_for(10_000, || async { app.state.queue.dead_letter_size() == 1 }).await;
    assert!(dead, "exhausted media job lands in the dead-letter queue");
    let stats = app.state.queue.stats.lock().unwrap().clone();
    assert!(stats.retries >= 2, "transient failures were retried: {stats:?}");
    assert!(stats.errors >= 3);
}

// ---------------------------------------------------------------- webhook hook

#[tokio::test]
async fn inbound_line_media_message_enqueues_processing() {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use tower::ServiceExt;

    let app = spawn_app().await;
    worker::spawn(app.state.clone());

    // Mirror the media content so the stubbed fetch succeeds.
    mcss_backend::domain::files::store::put_object(
        &app.state.config.upload_dir, "line/media/img-mid-1", PNG,
    )
    .await
    .unwrap();

    let body = json!({
        "destination": "d",
        "events": [{
            "type": "message", "timestamp": 1,
            "source": {"userId": "U-media"},
            "message": {"id": "img-mid-1", "type": "image"}
        }]
    })
    .to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"test-line-secret").unwrap();
    mac.update(body.as_bytes());
    let sig = B64.encode(mac.finalize().into_bytes());
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/webhook")
        .header("Content-Type", "application/json")
        .header("x-line-signature", sig)
        .body(axum::body::Body::from(body))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The message persists immediately; the attachment arrives via the queue.
    let attached = wait_for(5000, || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM attachments a JOIN messages m ON m.id = a.message_id
             WHERE m.platform_message_id = 'img-mid-1'",
        )
        .fetch_one(&app.state.db)
        .await
        .map(|c| c == 1)
        .unwrap_or(false)
    })
    .await;
    assert!(attached, "webhook media was queued and processed in the background");
}
