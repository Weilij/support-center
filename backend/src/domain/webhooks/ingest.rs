//! Inbound ingestion pipeline (CRD §4.2): deduplicate by platform message id,
//! find-or-create customer and open conversation, persist the message, record
//! the auto-reply idempotency ledger, and defer the non-critical follow-up
//! work (real-time broadcast, latest-message refresh, activity log).

use serde_json::{json, Map, Value};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::db::now_iso;
use crate::domain::auth::store::log_activity;
use crate::state::AppState;

use super::parse::Normalized;

/// Sentinel actor for webhook-originated audit entries (soft-deleted so it
/// never appears in member listings; mirrors the `deleted-user` precedent).
pub const SYSTEM_AGENT_ID: &str = "system";

/// Default localized welcome message stored when no welcome rule matches a
/// follow event, so the conversation is not empty (CRD 2824).
pub const DEFAULT_WELCOME: &str = "สวัสดีค่ะ ขอบคุณที่เพิ่มเราเป็นเพื่อน หากมีคำถามสามารถพิมพ์สอบถามได้เลยค่ะ";

async fn ensure_system_agent(db: &PgPool) {
    let now = now_iso();
    let _ = sqlx::query(
        "INSERT INTO agents
            (id, email, password_hash, display_name, role, is_active, password_policy,
             deleted_at, created_at)
         VALUES ($1, 'system@system.local', '', 'System', 'agent', 0, 'unchangeable', $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(SYSTEM_AGENT_ID)
    .bind(&now)
    .bind(&now)
    .execute(db)
    .await;
}

/// Per-customer conversation-creation serialization (CRD 2790): concurrent
/// inbound deliveries for the same customer never produce duplicate open
/// conversations.
fn customer_lock(key: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let map = LOCKS.get_or_init(Default::default);
    let mut guard = map.lock().expect("customer lock map");
    guard.entry(key.to_string()).or_default().clone()
}

async fn push_default_welcome(state: &AppState, message_id: &str, user_id: &str) {
    let gateway = crate::domain::conversations::channels::OutboundGateway::from_state(state);
    let item = crate::domain::conversations::channels::OutboundItem::text(DEFAULT_WELCOME);
    let now = now_iso();
    match gateway.send_batch("line", user_id, &[item]).await {
        Ok(platform_message_id) => {
            let _ = sqlx::query(
                "UPDATE messages
                    SET is_sent = 1, sent_at = $1, delivery_status = 'sent',
                        platform_message_id = $2, updated_at = $3
                 WHERE id = $4",
            )
            .bind(&now)
            .bind(platform_message_id)
            .bind(&now)
            .bind(message_id)
            .execute(&state.db)
            .await;
        }
        Err(error) => {
            tracing::warn!(error = %error, "LINE default welcome push failed");
            let _ = sqlx::query(
                "UPDATE messages
                    SET is_sent = 0, delivery_status = 'failed', updated_at = $1
                 WHERE id = $2",
            )
            .bind(&now)
            .bind(message_id)
            .execute(&state.db)
            .await;
        }
    }
}

/// Outcome of one inbound message ingestion.
#[derive(Debug)]
pub enum IngestOutcome {
    /// Redelivery of an already-seen platform message id: no side effects
    /// (CRD 2766, 2789).
    Duplicate,
    /// Event carried no resolvable end-user; ignored.
    Skipped,
    Created {
        customer_id: i64,
        conversation_id: String,
        message_id: String,
        team_id: Option<i64>,
    },
}

/// The default customer name used until a real profile is captured.
pub fn default_display_name(platform: &str) -> &'static str {
    match platform {
        "line" => "LINE User",
        "facebook" => "Facebook User",
        "instagram" => "Instagram User",
        "shopee" => "Shopee User",
        _ => "Customer",
    }
}

/// True when `name` is absent/blank or still this platform's placeholder — i.e.
/// no real profile has been captured for the customer yet.
pub fn is_placeholder_name(platform: &str, name: Option<&str>) -> bool {
    match name.map(str::trim) {
        None | Some("") => true,
        Some(n) => n == default_display_name(platform),
    }
}

pub struct InboundMessage<'a> {
    pub platform: &'a str,
    pub platform_user_id: &'a str,
    /// Used only when the customer record does not exist yet.
    pub default_display_name: &'a str,
    pub platform_message_id: Option<&'a str>,
    pub normalized: Normalized,
}

/// Reserve a non-message webhook event before running side effects. Returns
/// false when the exact event has already been processed.
pub async fn reserve_webhook_event(
    db: &PgPool,
    platform: &str,
    event_type: &str,
    event_key: &str,
) -> Result<bool, String> {
    if event_key.trim().is_empty() {
        return Ok(true);
    }
    let result = sqlx::query(
        "INSERT INTO webhook_replay_events (platform, event_type, event_key, seen_at)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (platform, event_type, event_key) DO NOTHING",
    )
    .bind(platform)
    .bind(event_type)
    .bind(event_key)
    .bind(now_iso())
    .execute(db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(result.rows_affected() == 1)
}

fn line_lifecycle_event_key(event_type: &str, event: &Value, user_id: &str) -> String {
    if let Some(id) = event
        .get("webhookEventId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return format!("webhook:{id}");
    }
    let timestamp = event
        .get("timestamp")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let reply_token = event
        .get("replyToken")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tracking = event
        .get("follow")
        .and_then(|f| f.get("trackingId").or_else(|| f.get("token")))
        .and_then(Value::as_str)
        .or_else(|| event.get("trackingToken").and_then(Value::as_str))
        .unwrap_or_default();
    format!("{event_type}:{user_id}:{timestamp}:{reply_token}:{tracking}")
}

#[derive(sqlx::FromRow)]
struct CustomerRow {
    id: i64,
    display_name: Option<String>,
    source_team_id: Option<i64>,
}

async fn find_customer(
    db: &PgPool,
    platform: &str,
    user_id: &str,
) -> Result<Option<CustomerRow>, String> {
    sqlx::query_as(
        "SELECT id, display_name, source_team_id FROM customers
         WHERE platform = $1 AND platform_user_id = $2 AND deleted_at IS NULL",
    )
    .bind(platform)
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(|e| e.to_string())
}

async fn find_or_create_customer(
    db: &PgPool,
    platform: &str,
    user_id: &str,
    display_name: &str,
) -> Result<CustomerRow, String> {
    if let Some(c) = find_customer(db, platform, user_id).await? {
        return Ok(c);
    }
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO customers (platform, platform_user_id, display_name, created_at)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(platform)
    .bind(user_id)
    .bind(display_name)
    .bind(now_iso())
    .fetch_one(db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(CustomerRow {
        id,
        display_name: Some(display_name.into()),
        source_team_id: None,
    })
}

/// Most recent customer-to-team routing assignment wins (CRD 2845-2846).
async fn routed_team(db: &PgPool, platform: &str, user_id: &str) -> Result<Option<i64>, String> {
    let _ = platform; // assignments are keyed by platform user id alone (CRD 5747)
    sqlx::query_scalar(
        "SELECT team_id FROM customer_team_assignments
         WHERE platform_user_id = $1
         ORDER BY assigned_at DESC, id DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(|e| e.to_string())
}

/// Find the customer's open (non-closed) conversation or create a new active
/// one; backfills a missing team assignment but never removes one (CRD 2849).
async fn find_or_create_open_conversation(
    db: &PgPool,
    customer_id: i64,
    team_id: Option<i64>,
) -> Result<(String, Option<i64>), String> {
    let existing: Option<(String, Option<i64>)> = sqlx::query_as(
        "SELECT id, team_id FROM conversations
         WHERE customer_id = $1 AND status != 'closed' AND deleted_at IS NULL
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(customer_id)
    .fetch_optional(db)
    .await
    .map_err(|e| e.to_string())?;

    if let Some((id, existing_team)) = existing {
        if existing_team.is_none() {
            if let Some(t) = team_id {
                sqlx::query("UPDATE conversations SET team_id = $1, updated_at = $2 WHERE id = $3")
                    .bind(t)
                    .bind(now_iso())
                    .bind(&id)
                    .execute(db)
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok((id, Some(t)));
            }
        }
        return Ok((id, existing_team));
    }

    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO conversations (id, customer_id, team_id, status, priority, created_at)
         VALUES ($1, $2, $3, 'active', 'normal', $4)",
    )
    .bind(&id)
    .bind(customer_id)
    .bind(team_id)
    .bind(now_iso())
    .execute(db)
    .await
    .map_err(|e| e.to_string())?;
    Ok((id, team_id))
}

/// Successful inbound messages increment the matching enabled connection's
/// received counter and last-message timestamp (CRD 2722).
async fn bump_received(db: &PgPool, team_id: i64, platform: &str, now: &str) {
    let row: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT id, stats FROM channel_integrations
         WHERE team_id = $1 AND platform = $2 AND is_active = 1 LIMIT 1",
    )
    .bind(team_id)
    .bind(platform)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    let Some((id, stats)) = row else { return };
    let mut parsed: Value = stats
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| json!({}));
    let received = parsed
        .get("messagesReceived")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    parsed["messagesReceived"] = json!(received + 1);
    parsed["lastMessageAt"] = json!(now);
    let _ =
        sqlx::query("UPDATE channel_integrations SET stats = $1, updated_at = $2 WHERE id = $3")
            .bind(parsed.to_string())
            .bind(now)
            .bind(id)
            .execute(db)
            .await;
}

/// Ingest one normalized inbound message end-to-end (CRD 2761-2791).
pub async fn ingest_message(
    state: &Arc<AppState>,
    inbound: InboundMessage<'_>,
) -> Result<IngestOutcome, String> {
    if inbound.platform_user_id.is_empty() {
        return Ok(IngestOutcome::Skipped);
    }

    // Per-platform-message-id idempotency: already-seen identifiers are
    // skipped entirely, with no customer/conversation side effects (CRD 2766).
    if let Some(mid) = inbound.platform_message_id {
        let seen: Option<String> =
            sqlx::query_scalar("SELECT id FROM messages WHERE platform_message_id = $1")
                .bind(mid)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| e.to_string())?;
        if seen.is_some() {
            // Redelivery recovery (CRD 1430-1433): the ledger's duplicate
            // guard keeps the auto-reply at-most-once.
            let _ = crate::domain::auto_reply::engine::retry_redelivered(
                state,
                inbound.platform,
                mid,
                inbound.platform_user_id,
                None,
            )
            .await;
            return Ok(IngestOutcome::Duplicate);
        }
    }

    let lock = customer_lock(&format!(
        "{}:{}",
        inbound.platform, inbound.platform_user_id
    ));
    let _guard = lock.lock().await;

    let mut customer = find_or_create_customer(
        &state.db,
        inbound.platform,
        inbound.platform_user_id,
        inbound.default_display_name,
    )
    .await?;

    // Fill the real name + avatar while we still only have the placeholder
    // (covers brand-new customers and old "<Platform> User" records). Best-effort:
    // a failed/absent profile leaves the placeholder untouched (CRD 2818).
    if is_placeholder_name(inbound.platform, customer.display_name.as_deref()) {
        let gateway =
            crate::domain::conversations::channels::OutboundGateway::from_config(&state.config);
        let profile = gateway
            .fetch_profile(inbound.platform, inbound.platform_user_id)
            .await;
        if profile.display_name.is_some() || profile.avatar_url.is_some() {
            let _ = sqlx::query(
                "UPDATE customers
                    SET display_name = COALESCE($1, display_name),
                        avatar_url   = COALESCE($2, avatar_url),
                        updated_at   = $3
                  WHERE id = $4",
            )
            .bind(profile.display_name.as_deref())
            .bind(profile.avatar_url.as_deref())
            .bind(now_iso())
            .bind(customer.id)
            .execute(&state.db)
            .await;
            if let Some(name) = profile.display_name {
                customer.display_name = Some(name);
            }
        }
    }
    let team_id = match routed_team(&state.db, inbound.platform, inbound.platform_user_id).await? {
        Some(t) => Some(t),
        None => customer.source_team_id,
    };
    let (conversation_id, team_id) =
        find_or_create_open_conversation(&state.db, customer.id, team_id).await?;

    let now = now_iso();
    let message_id = crate::domain::messaging::store::new_message_id();
    let mut metadata = Map::new();
    metadata.insert("platform".into(), json!(inbound.platform));
    metadata.insert("source".into(), json!("webhook"));
    if let Some(media) = &inbound.normalized.media {
        metadata.insert("media".into(), media.clone());
    }
    for (k, v) in &inbound.normalized.metadata {
        metadata.insert(k.clone(), v.clone());
    }
    let metadata_json = Value::Object(metadata).to_string();

    // Inbound messages are recorded as delivered (CRD 2851).
    let insert = sqlx::query(
        "INSERT INTO messages
            (id, conversation_id, sender_type, customer_id, content, content_type,
             platform_message_id, is_sent, sent_at, delivery_status, metadata, sender_name,
             created_at)
         VALUES ($1, $2, 'customer', $3, $4, $5, $6, 1, $7, 'delivered', $8, $9, $10)",
    )
    .bind(&message_id)
    .bind(&conversation_id)
    .bind(customer.id)
    .bind(&inbound.normalized.content)
    .bind(&inbound.normalized.kind)
    .bind(inbound.platform_message_id)
    .bind(&now)
    .bind(&metadata_json)
    .bind(
        customer
            .display_name
            .as_deref()
            .unwrap_or(inbound.default_display_name),
    )
    .bind(&now)
    .execute(&state.db)
    .await;

    if let Err(e) = insert {
        // A unique-constraint race on insert resolves to the existing record
        // rather than erroring (CRD 2789); the activity marker does not
        // advance on a pure redelivery (CRD 2791).
        if let Some(mid) = inbound.platform_message_id {
            let existing: Option<String> =
                sqlx::query_scalar("SELECT id FROM messages WHERE platform_message_id = $1")
                    .bind(mid)
                    .fetch_optional(&state.db)
                    .await
                    .map_err(|e2| e2.to_string())?;
            if existing.is_some() {
                return Ok(IngestOutcome::Duplicate);
            }
        }
        return Err(e.to_string());
    }

    // The most-recent-activity marker advances only when a row was actually
    // inserted (CRD 2791).
    let _ =
        sqlx::query("UPDATE conversations SET last_message_at = $1, updated_at = $2 WHERE id = $3")
            .bind(&now)
            .bind(&now)
            .bind(&conversation_id)
            .execute(&state.db)
            .await;

    if let Some(t) = team_id {
        bump_received(&state.db, t, inbound.platform, &now).await;
    }

    // Auto-reply evaluation runs synchronously because the platform reply
    // credential is short-lived (CRD 2742); the engine owns the idempotency
    // ledger (auto_reply_deliveries UNIQUE(platform, mid), CRD 1422).
    let _ = crate::domain::auto_reply::engine::evaluate_message(
        state,
        crate::domain::auto_reply::engine::MessageEvalInput {
            platform: inbound.platform,
            content: &inbound.normalized.content,
            message_type: &inbound.normalized.kind,
            conversation_id: &conversation_id,
            team_id,
            customer_id: customer.id,
            platform_user_id: inbound.platform_user_id,
            platform_message_id: inbound.platform_message_id,
            reply_token: None,
        },
    )
    .await;

    // Non-critical follow-up work is deferred to run after the HTTP response
    // (CRD 2768): real-time broadcast, latest-message refresh, activity log.
    spawn_followups(
        state.clone(),
        FollowupContext {
            platform: inbound.platform.to_string(),
            conversation_id: conversation_id.clone(),
            message_id: message_id.clone(),
            customer_id: customer.id,
            team_id,
            content: inbound.normalized.content.clone(),
            kind: inbound.normalized.kind.clone(),
            media: inbound.normalized.media.clone(),
            now,
        },
    );

    Ok(IngestOutcome::Created {
        customer_id: customer.id,
        conversation_id,
        message_id,
        team_id,
    })
}

struct FollowupContext {
    platform: String,
    conversation_id: String,
    message_id: String,
    customer_id: i64,
    team_id: Option<i64>,
    content: String,
    kind: String,
    media: Option<Value>,
    now: String,
}

fn spawn_followups(state: Arc<AppState>, ctx: FollowupContext) {
    tokio::spawn(async move {
        // New-message event for every successfully persisted inbound message
        // (CRD 2855): conversation audience plus team audience, source marked
        // as the webhook.
        let payload = json!({
            "conversationId": ctx.conversation_id,
            "message": {
                "id": ctx.message_id,
                "content": ctx.content,
                "type": ctx.kind,
                "senderType": "customer",
                "senderId": ctx.customer_id,
                "platform": ctx.platform,
                "timestamp": ctx.now,
                "deliveryStatus": "delivered",
                "metadata": ctx.media.as_ref().map(|m| m.to_string()),
            },
            "source": "webhook",
        });
        state
            .realtime
            .to_conversation(&ctx.conversation_id, "new_message", payload.clone());
        if let Some(t) = ctx.team_id {
            state.realtime.to_team(t, "new_message", payload);
        }

        // Latest-message cache refresh (CRD 2769).
        crate::realtime::latest::schedule_refresh(state.clone(), ctx.conversation_id.clone());

        // Media retrieval is queued for background processing (CRD §6.5,
        // 5133-5148); location/sticker kinds are non-downloadable.
        if let Some(media) = &ctx.media {
            let downloadable = matches!(ctx.kind.as_str(), "image" | "video" | "audio" | "file");
            let platform_mid = media
                .get("mediaId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if downloadable && !platform_mid.is_empty() {
                state.queue.enqueue_media(serde_json::json!({
                    "type": "media_processing",
                    "messageId": ctx.message_id,
                    "conversationId": ctx.conversation_id,
                    "teamId": ctx.team_id,
                    "platformMessageId": platform_mid,
                    "mediaType": ctx.kind,
                    "fileName": media.get("fileName"),
                    "enqueuedAt": chrono::Utc::now().timestamp_millis(),
                }));
            }
        }
        // TODO(notifications): new-conversation notification trigger (CRD 2860).

        ensure_system_agent(&state.db).await;
        log_activity(
            &state.db,
            SYSTEM_AGENT_ID,
            "System",
            "system",
            "webhook message received",
            "webhook",
            Some(&ctx.message_id),
            Some(json!({
                "platform": ctx.platform,
                "conversationId": ctx.conversation_id,
                "customerId": ctx.customer_id,
            })),
            None,
            None,
        )
        .await;
    });
}

// ------------------------------------------------------------ delivery / read receipts

/// Delivery receipt: mark messages delivered by their platform message ids.
pub async fn mark_delivered(db: &PgPool, mids: &[&str]) {
    for mid in mids {
        if let Err(e) = sqlx::query(
            "UPDATE messages SET delivery_status = 'delivered', updated_at = $1 WHERE platform_message_id = $2",
        )
        .bind(now_iso())
        .bind(mid)
        .execute(db)
        .await
        {
            tracing::warn!(error = %e, "facebook delivery receipt update failed");
        }
    }
}

/// Convert an epoch-millis read watermark to the canonical ISO-8601 form used
/// for every TEXT timestamp column (matching `crate::db::now_iso`: millisecond
/// precision, `Z`-suffixed UTC). Read-receipt comparisons happen as TEXT, so the
/// watermark MUST be byte-comparable with the `sent_at` values written by
/// `now_iso`; the older `to_rfc3339()` `+00:00` form sorts before the `.000Z`
/// form under byte-ordered collations, silently dropping same-second receipts.
pub fn watermark_to_iso(watermark_ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(watermark_ms)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// Read receipt: stamp `read_at` on the customer's agent messages sent at or
/// before the watermark (ms epoch). FB read events carry no message ids.
pub async fn mark_read(db: &PgPool, platform: &str, platform_user_id: &str, watermark_ms: i64) {
    let Some(iso) = watermark_to_iso(watermark_ms) else {
        return;
    };
    if let Err(e) = sqlx::query(
        "UPDATE messages SET read_at = $1
         WHERE sender_type = 'agent' AND read_at IS NULL AND sent_at <= $2
           AND conversation_id IN (
             SELECT c.id FROM conversations c
             JOIN customers cu ON cu.id = c.customer_id
             WHERE cu.platform = $3 AND cu.platform_user_id = $4
           )",
    )
    .bind(now_iso())
    .bind(&iso)
    .bind(platform)
    .bind(platform_user_id)
    .execute(db)
    .await
    {
        tracing::warn!(error = %e, "facebook read receipt update failed");
    }
}

/// IG/FB message reaction: update the target message's `metadata.reactions`.
pub async fn apply_reaction(db: &PgPool, reaction: &serde_json::Value) {
    let Some(mid) = reaction.get("mid").and_then(serde_json::Value::as_str) else {
        return;
    };
    let action = reaction
        .get("action")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("react");
    let react_type = reaction.get("reaction").and_then(serde_json::Value::as_str);
    let emoji = reaction.get("emoji").and_then(serde_json::Value::as_str);

    let found: Option<Option<String>> =
        sqlx::query_scalar("SELECT metadata FROM messages WHERE platform_message_id = $1")
            .bind(mid)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    let Some(meta_text) = found else { return };
    let mut meta: serde_json::Value = meta_text
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    if !meta.is_object() {
        meta = json!({});
    }
    let arr = meta
        .as_object_mut()
        .unwrap()
        .entry("reactions")
        .or_insert_with(|| json!([]));
    if let Some(list) = arr.as_array_mut() {
        if action == "unreact" {
            list.retain(|r| r.get("reaction").and_then(serde_json::Value::as_str) != react_type);
        } else {
            list.push(json!({ "reaction": react_type, "emoji": emoji }));
        }
    }
    if let Err(e) = sqlx::query(
        "UPDATE messages SET metadata = $1, updated_at = $2 WHERE platform_message_id = $3",
    )
    .bind(meta.to_string())
    .bind(now_iso())
    .bind(mid)
    .execute(db)
    .await
    {
        tracing::warn!(error = %e, "reaction metadata update failed");
    }
}

/// Read receipt keyed by a specific message id (IG "seen" may carry `read.mid`):
/// mark agent messages up to that message's sent_at as read.
pub async fn mark_read_by_mid(db: &PgPool, platform: &str, platform_user_id: &str, mid: &str) {
    let at: Option<Option<String>> =
        sqlx::query_scalar("SELECT sent_at FROM messages WHERE platform_message_id = $1")
            .bind(mid)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    let Some(Some(sent_at)) = at else { return };
    if let Err(e) = sqlx::query(
        "UPDATE messages SET read_at = $1
         WHERE sender_type = 'agent' AND read_at IS NULL AND sent_at <= $2
           AND conversation_id IN (
             SELECT c.id FROM conversations c
             JOIN customers cu ON cu.id = c.customer_id
             WHERE cu.platform = $3 AND cu.platform_user_id = $4
           )",
    )
    .bind(now_iso())
    .bind(&sent_at)
    .bind(platform)
    .bind(platform_user_id)
    .execute(db)
    .await
    {
        tracing::warn!(error = %e, "read-by-mid update failed");
    }
}

// ------------------------------------------------------------ follow / unfollow (LINE)

/// Follow / opt-in lifecycle handling (CRD 2814-2826).
pub async fn handle_line_follow(state: &Arc<AppState>, event: &Value) -> Result<(), String> {
    let Some(user_id) = event["source"]["userId"].as_str().filter(|s| !s.is_empty()) else {
        return Ok(()); // absent end-user identifier: silently ignored (CRD 2826)
    };
    let event_key = line_lifecycle_event_key("follow", event, user_id);
    if !reserve_webhook_event(&state.db, "line", "follow", &event_key).await? {
        return Ok(());
    }
    let now = now_iso();

    // Capture the real profile (name + avatar) on follow when we still only have
    // a placeholder; failure is tolerated and the previous/default name is used
    // (CRD 2818).
    let existing = find_customer(&state.db, "line", user_id).await?;
    let stored = existing.as_ref().and_then(|c| c.display_name.clone());
    let mut display_name = stored
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| default_display_name("line").to_string());
    let mut avatar_url: Option<String> = None;
    if is_placeholder_name("line", stored.as_deref()) {
        let gateway =
            crate::domain::conversations::channels::OutboundGateway::from_config(&state.config);
        let profile = gateway.fetch_profile("line", user_id).await;
        if let Some(name) = profile.display_name {
            display_name = name;
        }
        avatar_url = profile.avatar_url;
    }

    // Tracking parameters that may carry a team-routing token (CRD 2817).
    let tracking_token = event["follow"]["trackingId"]
        .as_str()
        .or_else(|| event["follow"]["token"].as_str())
        .or_else(|| event["trackingToken"].as_str())
        .filter(|s| !s.is_empty());

    // Team resolution priority: stored routing assignment, then tracking-token
    // lookup (CRD 2819).
    let mut routed_via_tracking = false;
    let team_id = match routed_team(&state.db, "line", user_id).await? {
        Some(t) => Some(t),
        None => match tracking_token {
            Some(token) => {
                let resolved: Option<i64> = sqlx::query_scalar(
                    "SELECT team_id FROM qr_codes WHERE token = $1 AND is_active = 1 LIMIT 1",
                )
                .bind(token)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| e.to_string())?;
                routed_via_tracking = resolved.is_some();
                resolved
            }
            None => None,
        },
    };

    // Create-if-absent, then update profile + follow metadata (CRD 2820).
    let customer = find_or_create_customer(&state.db, "line", user_id, &display_name).await?;
    let mut meta = json!({ "lastFollowedAt": now });
    if routed_via_tracking {
        meta["assignedViaTracking"] = json!(true);
    }
    let _ = sqlx::query(
        "UPDATE customers SET display_name = $1,
                avatar_url = COALESCE($2, avatar_url),
                metadata = (COALESCE(metadata, '{}')::jsonb || $3::jsonb)::text,
                updated_at = $4
         WHERE id = $5",
    )
    .bind(&display_name)
    .bind(avatar_url.as_deref())
    .bind(meta.to_string())
    .bind(&now)
    .bind(customer.id)
    .execute(&state.db)
    .await;

    if routed_via_tracking {
        if let Some(t) = team_id {
            let _ = sqlx::query(
                "INSERT INTO customer_team_assignments
                    (id, platform_user_id, team_id, source, assigned_at)
                 VALUES ($1, $2, $3, 'inbound', $4)
                 ON CONFLICT (platform_user_id, team_id)
                 DO UPDATE SET source = 'inbound', assigned_at = EXCLUDED.assigned_at",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(user_id)
            .bind(t)
            .bind(&now)
            .execute(&state.db)
            .await;
        }
    }

    // With a resolved team: reuse a non-closed conversation (team-backfilled)
    // or create a new active, normal-priority one (CRD 2821).
    let mut conversation_id: Option<String> = None;
    if team_id.is_some() {
        let lock = customer_lock(&format!("line:{user_id}"));
        let _guard = lock.lock().await;
        let (cid, _) = find_or_create_open_conversation(&state.db, customer.id, team_id).await?;
        conversation_id = Some(cid);
    }

    // Real-time assignment/transfer event with reconciliation markers
    // (CRD 2822, 2857).
    if let (Some(t), Some(cid)) = (team_id, conversation_id.as_ref()) {
        let team_name: Option<String> = sqlx::query_scalar("SELECT name FROM teams WHERE id = $1")
            .bind(t)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
        state.realtime.to_teams_and_admins(
            &[t],
            "conversation_transferred",
            json!({
                "fromTeamId": null,
                "toTeamId": t,
                "teamName": team_name,
                "conversation": {
                    "id": cid,
                    "customerId": customer.id,
                    "status": "active",
                    "priority": "normal",
                },
                "platformUserId": user_id,
                "webhookConfirmed": true,
                "timestamp": now,
            }),
        );
    }

    // Welcome auto-reply, attempted before notifications because the reply
    // credential is short-lived (CRD 2823): welcome rules first, falling back
    // to the default localized welcome only when no rule matched (CRD 2822).
    let mut welcome_sent = false;
    if let Some(cid) = conversation_id.as_ref() {
        let result = crate::domain::auto_reply::engine::evaluate_welcome(
            state,
            "line",
            team_id,
            cid,
            customer.id,
            user_id,
            event["replyToken"].as_str(),
        )
        .await;
        welcome_sent = result.sent;
    }
    if let (Some(cid), false) = (conversation_id.as_ref(), welcome_sent) {
        let welcome_id = crate::domain::messaging::store::new_message_id();
        let inserted = sqlx::query(
            "INSERT INTO messages
                (id, conversation_id, sender_type, content, content_type, is_sent, sent_at,
                 delivery_status, sender_name, created_at)
             VALUES ($1, $2, 'system', $3, 'text', 0, NULL, 'pending', 'System', $4)",
        )
        .bind(&welcome_id)
        .bind(cid)
        .bind(DEFAULT_WELCOME)
        .bind(&now)
        .execute(&state.db)
        .await;
        if inserted.is_ok() {
            push_default_welcome(state, &welcome_id, user_id).await;
            let _ = sqlx::query(
                "UPDATE conversations SET last_message_at = $1, updated_at = $2 WHERE id = $3",
            )
            .bind(&now)
            .bind(&now)
            .bind(cid)
            .execute(&state.db)
            .await;
        }
    }

    ensure_system_agent(&state.db).await;
    log_activity(
        &state.db,
        SYSTEM_AGENT_ID,
        "System",
        "system",
        "customer follow",
        "customer",
        Some(&customer.id.to_string()),
        Some(json!({
            "platform": "line",
            "platformUserId": user_id,
            "teamId": team_id,
            "assignedViaTracking": routed_via_tracking,
        })),
        None,
        None,
    )
    .await;
    // TODO(notifications): customer-followed notification trigger (CRD 2825).

    Ok(())
}

/// Unfollow / opt-out lifecycle handling (CRD 2828-2833).
pub async fn handle_line_unfollow(state: &Arc<AppState>, event: &Value) -> Result<(), String> {
    let Some(user_id) = event["source"]["userId"].as_str().filter(|s| !s.is_empty()) else {
        return Ok(()); // absent identifier: ignored (CRD 2833)
    };
    let event_key = line_lifecycle_event_key("unfollow", event, user_id);
    if !reserve_webhook_event(&state.db, "line", "unfollow", &event_key).await? {
        return Ok(());
    }
    let Some(customer) = find_customer(&state.db, "line", user_id).await? else {
        return Ok(()); // no customer found: the event is a no-op (CRD 2831)
    };

    let now = now_iso();
    sqlx::query("UPDATE customers SET updated_at = $1 WHERE id = $2")
        .bind(&now)
        .bind(customer.id)
        .execute(&state.db)
        .await
        .map_err(|e| e.to_string())?;

    // Activity entry; failure tolerated (CRD 2832).
    ensure_system_agent(&state.db).await;
    log_activity(
        &state.db,
        SYSTEM_AGENT_ID,
        "System",
        "system",
        "customer unfollow",
        "customer",
        Some(&customer.id.to_string()),
        Some(json!({ "platform": "line", "platformUserId": user_id })),
        None,
        None,
    )
    .await;
    Ok(())
}

#[cfg(test)]
mod placeholder_tests {
    use super::{default_display_name, is_placeholder_name, line_lifecycle_event_key};
    use serde_json::json;

    #[test]
    fn defaults_per_platform() {
        assert_eq!(default_display_name("line"), "LINE User");
        assert_eq!(default_display_name("facebook"), "Facebook User");
        assert_eq!(default_display_name("instagram"), "Instagram User");
        assert_eq!(default_display_name("shopee"), "Shopee User");
    }

    #[test]
    fn placeholder_detection() {
        assert!(is_placeholder_name("line", None));
        assert!(is_placeholder_name("line", Some("")));
        assert!(is_placeholder_name("line", Some("   ")));
        assert!(is_placeholder_name("line", Some("LINE User")));
        assert!(is_placeholder_name("facebook", Some("Facebook User")));
        assert!(!is_placeholder_name("line", Some("陳小明")));
        // A real name that happens to match another platform's placeholder is
        // still real for this platform.
        assert!(!is_placeholder_name("line", Some("Facebook User")));
    }

    #[test]
    fn line_lifecycle_event_key_prefers_webhook_event_id() {
        let event = json!({
            "webhookEventId": "01HABC",
            "timestamp": 123,
            "replyToken": "reply",
            "source": { "userId": "U1" }
        });

        assert_eq!(
            line_lifecycle_event_key("follow", &event, "U1"),
            "webhook:01HABC"
        );
    }

    #[test]
    fn line_lifecycle_event_key_has_stable_fallback() {
        let event = json!({
            "timestamp": 123,
            "replyToken": "reply",
            "source": { "userId": "U1" },
            "follow": { "trackingId": "track" }
        });

        assert_eq!(
            line_lifecycle_event_key("follow", &event, "U1"),
            "follow:U1:123:reply:track"
        );
    }
}
