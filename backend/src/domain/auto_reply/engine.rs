//! Auto-reply evaluation & dispatch (CRD §2.5, lines 1412-1433).
//!
//! Triggered internally by inbound-message and follow webhook processing.
//! Evaluation never throws to the caller: errors yield a non-matched result
//! carrying an error description (CRD 1421).

use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::db::now_iso;
use crate::domain::conversations::channels::{OutboundGateway, OutboundItem};
use crate::state::AppState;

const CACHE_TTL: Duration = Duration::from_secs(60);

/// (cached-at, rules) per scope; scope None = global.
type CacheEntries = HashMap<Option<i64>, (Instant, Vec<CachedRule>)>;
/// content, content_type, conversation_id, team_id, customer_id
type OriginalMessageRow = (String, String, String, Option<i64>, Option<i64>);

#[derive(Debug, Clone)]
pub struct CachedCondition {
    pub condition_type: String, // exact | contains | regex | message_type
    pub value: String,
    pub case_sensitive: bool,
    pub match_mode: String, // any | all (shared by the rule's conditions)
}

#[derive(Debug, Clone)]
pub struct CachedAction {
    pub action_type: String, // text | image | flex
    pub content: String,     // JSON string payload
}

#[derive(Debug, Clone)]
pub struct CachedRule {
    pub id: i64,
    pub team_id: Option<i64>,
    pub name: String,
    pub trigger_type: String, // welcome | keyword | off_hours | fallback
    pub priority: i64,
    pub allow_fallback: bool,
    pub conditions: Vec<CachedCondition>,
    pub actions: Vec<CachedAction>,
}

/// Per-scope (team or global) rule cache with explicit invalidation
/// (CRD 1354, 1451).
#[derive(Default)]
pub struct RuleCache {
    entries: Mutex<CacheEntries>,
}

impl RuleCache {
    fn get(&self, scope: &Option<i64>) -> Option<Vec<CachedRule>> {
        let entries = self.entries.lock().ok()?;
        let (at, rules) = entries.get(scope)?;
        (at.elapsed() < CACHE_TTL).then(|| rules.clone())
    }

    fn put(&self, scope: Option<i64>, rules: Vec<CachedRule>) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(scope, (Instant::now(), rules));
        }
    }

    pub fn invalidate(&self, scope: Option<i64>) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(&scope);
        }
    }
}

/// Load the active, non-deleted rules of one scope (None = global), with
/// conditions and actions, ordered by priority ascending.
async fn load_scope(db: &PgPool, scope: Option<i64>) -> Result<Vec<CachedRule>, sqlx::Error> {
    let rows: Vec<(i64, Option<i64>, String, String, i64, i64)> = sqlx::query_as(
        "SELECT id, team_id, name, trigger_type, priority, allow_fallback
         FROM auto_reply_rules
         WHERE deleted_at IS NULL AND is_active = 1
           AND (($1 IS NULL AND team_id IS NULL) OR team_id = $2)
         ORDER BY priority ASC, id ASC",
    )
    .bind(scope)
    .bind(scope)
    .fetch_all(db)
    .await?;

    let mut rules = Vec::with_capacity(rows.len());
    for (id, team_id, name, trigger_type, priority, allow_fallback) in rows {
        let conditions: Vec<(String, Option<String>, i64, String)> = sqlx::query_as(
            "SELECT condition_type, value, case_sensitive, match_mode
             FROM auto_reply_conditions WHERE rule_id = $1 ORDER BY id ASC",
        )
        .bind(id)
        .fetch_all(db)
        .await?;
        let actions: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT action_type, content FROM auto_reply_actions
             WHERE rule_id = $1 ORDER BY sort_order ASC, id ASC",
        )
        .bind(id)
        .fetch_all(db)
        .await?;
        rules.push(CachedRule {
            id,
            team_id,
            name,
            trigger_type,
            priority,
            allow_fallback: allow_fallback != 0,
            conditions: conditions
                .into_iter()
                .map(|(t, v, cs, m)| CachedCondition {
                    condition_type: t,
                    value: v.unwrap_or_default(),
                    case_sensitive: cs != 0,
                    match_mode: m,
                })
                .collect(),
            actions: actions
                .into_iter()
                .map(|(t, c)| CachedAction {
                    action_type: t,
                    content: c.unwrap_or_default(),
                })
                .collect(),
        });
    }
    Ok(rules)
}

/// Global + team rules merged in priority order; at equal priority a
/// team-specific rule precedes a global one (CRD 1414).
async fn applicable_rules(
    state: &AppState,
    team_id: Option<i64>,
) -> Result<Vec<CachedRule>, sqlx::Error> {
    let mut merged = match state.auto_reply_cache.get(&None) {
        Some(r) => r,
        None => {
            let r = load_scope(&state.db, None).await?;
            state.auto_reply_cache.put(None, r.clone());
            r
        }
    };
    if let Some(t) = team_id {
        let team_rules = match state.auto_reply_cache.get(&Some(t)) {
            Some(r) => r,
            None => {
                let r = load_scope(&state.db, Some(t)).await?;
                state.auto_reply_cache.put(Some(t), r.clone());
                r
            }
        };
        merged.extend(team_rules);
    }
    merged.sort_by_key(|r| (r.priority, r.team_id.is_none()));
    Ok(merged)
}

// ------------------------------------------------------------ business hours

/// "Within business hours" per CRD 1446: true when the current time in the
/// schedule's timezone falls inside an active entry for the current weekday;
/// close-at-or-before-open windows cross midnight; NO schedule entries at all
/// means always within hours (suppressing off-hours rules).
pub async fn within_business_hours(db: &PgPool, team_id: Option<i64>) -> bool {
    let Some(team) = team_id else { return true };
    let rows: Vec<(i64, String, String, String)> = match sqlx::query_as(
        "SELECT day_of_week, start_time, end_time, COALESCE(timezone, 'Asia/Taipei')
         FROM auto_reply_business_hours WHERE team_id = $1 AND is_active = 1",
    )
    .bind(team)
    .fetch_all(db)
    .await
    {
        Ok(r) => r,
        Err(_) => return true,
    };
    if rows.is_empty() {
        return true;
    }
    let tz: chrono_tz::Tz = rows[0].3.parse().unwrap_or(chrono_tz::Asia::Taipei);
    let now = chrono::Utc::now().with_timezone(&tz);
    let weekday = now.format("%w").to_string().parse::<i64>().unwrap_or(0); // 0 = Sunday
    let hm = now.format("%H:%M").to_string();
    for (day, start, end, _) in &rows {
        if *day != weekday {
            continue;
        }
        let crosses_midnight = end <= start;
        let inside = if crosses_midnight {
            hm >= *start || hm < *end
        } else {
            hm >= *start && hm < *end
        };
        if inside {
            return true;
        }
    }
    // An overnight window started yesterday may still be open this morning.
    let yesterday = (weekday + 6) % 7;
    for (day, start, end, _) in &rows {
        if *day == yesterday && end <= start && hm < *end {
            return true;
        }
    }
    false
}

// ------------------------------------------------------------ condition match

fn condition_matches(cond: &CachedCondition, text: &str, message_type: &str) -> bool {
    let (hay, needle) = if cond.case_sensitive {
        (text.to_string(), cond.value.clone())
    } else {
        (text.to_lowercase(), cond.value.to_lowercase())
    };
    match cond.condition_type.as_str() {
        "exact" => hay.trim() == needle.trim(),
        "contains" => hay.contains(&needle),
        // An invalid regular expression simply never matches (CRD 1437).
        "regex" => regex::RegexBuilder::new(&cond.value)
            .case_insensitive(!cond.case_sensitive)
            .build()
            .map(|re| re.is_match(text))
            .unwrap_or(false),
        "message_type" => message_type.eq_ignore_ascii_case(cond.value.trim()),
        _ => false,
    }
}

fn keyword_rule_matches(rule: &CachedRule, text: &str, message_type: &str) -> bool {
    // A keyword rule with no conditions can never match (CRD 1416).
    if rule.conditions.is_empty() {
        return false;
    }
    let all_mode = rule.conditions.iter().any(|c| c.match_mode == "all");
    if all_mode {
        rule.conditions
            .iter()
            .all(|c| condition_matches(c, text, message_type))
    } else {
        rule.conditions
            .iter()
            .any(|c| condition_matches(c, text, message_type))
    }
}

// ------------------------------------------------------------ dispatch

#[derive(Debug, Default)]
pub struct EvalResult {
    pub matched: bool,
    pub rule_id: Option<i64>,
    pub sent: bool,
    pub error: Option<String>,
}

enum DeliveryUpdateOutcome {
    Updated,
    Missing,
    Failed(String),
}

fn apply_delivery_update_outcome(
    mut result: EvalResult,
    platform: &str,
    platform_message_id: &str,
    outcome: DeliveryUpdateOutcome,
) -> EvalResult {
    match outcome {
        DeliveryUpdateOutcome::Updated => {}
        DeliveryUpdateOutcome::Missing => {
            let error = "auto-reply delivery ledger update affected no rows".to_string();
            tracing::warn!(
                platform,
                platform_message_id,
                "auto-reply delivery ledger update missed"
            );
            result.error.get_or_insert(error);
        }
        DeliveryUpdateOutcome::Failed(error) => {
            tracing::warn!(
                error = %error,
                platform,
                platform_message_id,
                "auto-reply delivery ledger update failed"
            );
            result.error.get_or_insert_with(|| {
                format!("auto-reply delivery ledger update failed: {error}")
            });
        }
    }
    result
}

/// Build the outbound items + a response summary from the rule's actions.
fn build_actions(rule: &CachedRule) -> Result<(Vec<OutboundItem>, String), String> {
    let mut items = Vec::new();
    let mut summary = Vec::new();
    for action in &rule.actions {
        let payload: Value = serde_json::from_str(&action.content).unwrap_or(Value::Null);
        let text = match action.action_type.as_str() {
            "text" => payload
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| payload.as_str().map(str::to_string))
                .unwrap_or_else(|| action.content.clone()),
            "image" => {
                let url = payload
                    .get("imageUrl")
                    .or_else(|| payload.get("originalContentUrl"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if url.is_empty() {
                    return Err("image action is missing an image URL".into());
                }
                format!("[Image] {url}")
            }
            "flex" => payload
                .get("altText")
                .and_then(Value::as_str)
                .unwrap_or("[Rich message]")
                .to_string(),
            other => return Err(format!("unsupported action type '{other}'")),
        };
        summary.push(text.clone());
        items.push(OutboundItem::text(text));
    }
    if items.is_empty() {
        return Err("rule has no valid actions".into());
    }
    Ok((items, summary.join("\n")))
}

struct DispatchContext<'a> {
    platform: &'a str,
    conversation_id: &'a str,
    team_id: Option<i64>,
    customer_id: i64,
    platform_user_id: &'a str,
    reply_token: Option<&'a str>,
    trigger_content: &'a str,
    matched_condition: Value,
}

/// Execute a matched rule's actions and apply the post-send side effects.
/// Each post-send step fails independently and non-fatally (CRD 1420).
async fn dispatch(state: &AppState, rule: &CachedRule, ctx: &DispatchContext<'_>) -> EvalResult {
    let (items, summary) = match build_actions(rule) {
        Ok(v) => v,
        Err(e) => {
            return EvalResult {
                matched: false,
                rule_id: Some(rule.id),
                sent: false,
                error: Some(e),
            }
        }
    };

    // Primary delivery uses the short-lived reply credential; the push
    // fallback is only permitted when the rule opted in (CRD 1436, 5795).
    let method = if ctx.reply_token.is_some() {
        "reply"
    } else if rule.allow_fallback {
        "push"
    } else {
        "reply"
    };
    let gateway = OutboundGateway::resolve(state).await;
    if let Err(e) = gateway
        .send_batch(ctx.platform, ctx.platform_user_id, &items)
        .await
    {
        return EvalResult {
            matched: true,
            rule_id: Some(rule.id),
            sent: false,
            error: Some(e.to_string()),
        };
    }
    let now = now_iso();

    // Stored system-authored conversation message (non-fatal).
    let message_id = uuid::Uuid::new_v4().to_string();
    let stored = sqlx::query(
        "INSERT INTO messages
            (id, conversation_id, sender_type, content, content_type, is_sent, sent_at,
             delivery_status, sender_name, metadata, created_at)
         VALUES ($1, $2, 'system', $3, 'text', 1, $4, 'delivered', 'Auto-Reply', $5, $6)",
    )
    .bind(&message_id)
    .bind(ctx.conversation_id)
    .bind(&summary)
    .bind(&now)
    .bind(json!({"autoReply": true, "ruleId": rule.id, "platform": ctx.platform}).to_string())
    .bind(&now)
    .execute(&state.db)
    .await;
    match stored {
        Ok(_) => {
            if let Err(error) = sqlx::query(
                "UPDATE conversations SET last_message_at = $1, updated_at = $2 WHERE id = $3",
            )
            .bind(&now)
            .bind(&now)
            .bind(ctx.conversation_id)
            .execute(&state.db)
            .await
            {
                tracing::warn!(
                    error = %error,
                    conversation_id = ctx.conversation_id,
                    message_id,
                    "auto-reply conversation timestamp update failed"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                conversation_id = ctx.conversation_id,
                message_id,
                "auto-reply conversation message insert failed"
            );
        }
    }

    // Audit log entry (non-fatal, append-only).
    if let Err(error) = sqlx::query(
        "INSERT INTO auto_reply_logs
            (rule_id, conversation_id, customer_id, trigger_content, response_content,
             matched_condition, platform, delivery_method, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(rule.id)
    .bind(ctx.conversation_id)
    .bind(ctx.customer_id)
    .bind(ctx.trigger_content)
    .bind(&summary)
    .bind(ctx.matched_condition.to_string())
    .bind(ctx.platform)
    .bind(method)
    .bind(&now)
    .execute(&state.db)
    .await
    {
        tracing::warn!(
            error = %error,
            rule_id = rule.id,
            conversation_id = ctx.conversation_id,
            "auto-reply audit log insert failed"
        );
    }

    // Real-time broadcast: the reply presented as a delivered auto-reply
    // message (agent-type sender for client compatibility, CRD 1449).
    state.realtime.to_conversation(
        ctx.conversation_id,
        "new_message",
        json!({
            "conversationId": ctx.conversation_id,
            "message": {
                "id": message_id,
                "content": summary,
                "type": "text",
                "senderType": "agent",
                "senderName": "Auto-Reply",
                "autoReply": true,
                "platform": ctx.platform,
                "timestamp": now,
                "deliveryStatus": "delivered",
            },
            "teamId": ctx.team_id,
        }),
    );

    EvalResult {
        matched: true,
        rule_id: Some(rule.id),
        sent: true,
        error: None,
    }
}

// ------------------------------------------------------------ entry points

pub struct MessageEvalInput<'a> {
    pub platform: &'a str,
    pub content: &'a str,
    pub message_type: &'a str,
    pub conversation_id: &'a str,
    pub team_id: Option<i64>,
    pub customer_id: i64,
    pub platform_user_id: &'a str,
    pub platform_message_id: Option<&'a str>,
    pub reply_token: Option<&'a str>,
}

/// Inbound-message evaluation (CRD 1412-1422). First match by priority wins.
pub async fn evaluate_message(state: &AppState, input: MessageEvalInput<'_>) -> EvalResult {
    let rules = match applicable_rules(state, input.team_id).await {
        Ok(r) => r,
        Err(e) => {
            return EvalResult {
                matched: false,
                rule_id: None,
                sent: false,
                error: Some(e.to_string()),
            }
        }
    };

    let mut winner: Option<&CachedRule> = None;
    for rule in &rules {
        let eligible = match rule.trigger_type.as_str() {
            // Greeting rules fire only via the follow path (CRD 1415).
            "welcome" | "greeting" => false,
            "keyword" => {
                input.message_type == "text"
                    && !input.content.trim().is_empty()
                    && keyword_rule_matches(rule, input.content, input.message_type)
            }
            "off_hours" => {
                let schedule_team = rule.team_id.or(input.team_id);
                !within_business_hours(&state.db, schedule_team).await
            }
            "fallback" => true,
            _ => false,
        };
        if eligible {
            winner = Some(rule);
            break;
        }
    }
    let Some(rule) = winner else {
        return EvalResult::default();
    };

    // Duplicate-send protection keyed by channel + platform message id
    // (CRD 1422); bypassed when no platform message identifier is supplied.
    if let Some(mid) = input.platform_message_id {
        match reserve_attempt(
            &state.db,
            input.platform,
            mid,
            rule.id,
            input.conversation_id,
            input.customer_id,
        )
        .await
        {
            Reservation::AlreadySucceeded => {
                return EvalResult {
                    matched: true,
                    rule_id: Some(rule.id),
                    sent: false,
                    error: None,
                }
            }
            Reservation::Pending => {
                return EvalResult {
                    matched: true,
                    rule_id: Some(rule.id),
                    sent: false,
                    error: Some("a delivery attempt is already pending".into()),
                }
            }
            Reservation::Reserved => {}
            Reservation::Error(e) => {
                return EvalResult {
                    matched: false,
                    rule_id: Some(rule.id),
                    sent: false,
                    error: Some(e),
                }
            }
        }
    }

    let matched_condition = if rule.trigger_type == "keyword" {
        json!(rule
            .conditions
            .iter()
            .map(|c| json!({"type": c.condition_type, "value": c.value}))
            .collect::<Vec<_>>())
    } else {
        json!({"trigger": rule.trigger_type})
    };
    let result = dispatch(
        state,
        rule,
        &DispatchContext {
            platform: input.platform,
            conversation_id: input.conversation_id,
            team_id: input.team_id,
            customer_id: input.customer_id,
            platform_user_id: input.platform_user_id,
            reply_token: input.reply_token,
            trigger_content: input.content,
            matched_condition,
        },
    )
    .await;

    if let Some(mid) = input.platform_message_id {
        let (status, err) = if result.sent {
            ("success", None)
        } else {
            ("failed", result.error.clone())
        };
        let delivery_update = sqlx::query(
            "UPDATE auto_reply_deliveries
             SET status = $1, rule_id = $2, delivery_method = $3, last_error = $4, sent_at = $5
             WHERE platform = $6 AND platform_message_id = $7",
        )
        .bind(status)
        .bind(rule.id)
        .bind(if input.reply_token.is_some() {
            "reply"
        } else {
            "push"
        })
        .bind(err)
        .bind(result.sent.then(now_iso))
        .bind(input.platform)
        .bind(mid)
        .execute(&state.db)
        .await;
        let outcome = match delivery_update {
            Ok(done) if done.rows_affected() > 0 => DeliveryUpdateOutcome::Updated,
            Ok(_) => DeliveryUpdateOutcome::Missing,
            Err(error) => DeliveryUpdateOutcome::Failed(error.to_string()),
        };
        return apply_delivery_update_outcome(result, input.platform, mid, outcome);
    }
    result
}

enum Reservation {
    Reserved,
    Pending,
    AlreadySucceeded,
    Error(String),
}

/// Ledger state machine (CRD 1445): none -> pending (reserved) -> success
/// (terminal) | failed (retryable, attempt counter incremented).
async fn reserve_attempt(
    db: &PgPool,
    platform: &str,
    mid: &str,
    rule_id: i64,
    conversation_id: &str,
    customer_id: i64,
) -> Reservation {
    let existing: Result<Option<String>, _> = sqlx::query_scalar(
        "SELECT status FROM auto_reply_deliveries WHERE platform = $1 AND platform_message_id = $2",
    )
    .bind(platform)
    .bind(mid)
    .fetch_optional(db)
    .await;
    match existing {
        Err(e) => Reservation::Error(e.to_string()),
        Ok(Some(status)) if status == "success" => Reservation::AlreadySucceeded,
        Ok(Some(status)) if status == "pending" => Reservation::Pending,
        Ok(Some(_failed)) => {
            let updated = sqlx::query(
                "UPDATE auto_reply_deliveries
                 SET status = 'pending', attempt_count = attempt_count + 1, last_attempt_at = $1
                 WHERE platform = $2 AND platform_message_id = $3 AND status = 'failed'",
            )
            .bind(now_iso())
            .bind(platform)
            .bind(mid)
            .execute(db)
            .await;
            match updated {
                Ok(r) if r.rows_affected() == 1 => Reservation::Reserved,
                Ok(_) => Reservation::Pending, // lost the race
                Err(e) => Reservation::Error(e.to_string()),
            }
        }
        Ok(None) => {
            let inserted = sqlx::query(
                "INSERT INTO auto_reply_deliveries
                    (platform, platform_message_id, rule_id, conversation_id, customer_id,
                     status, attempt_count, last_attempt_at, created_at)
                 VALUES ($1, $2, $3, $4, $5, 'pending', 1, $6, $7) ON CONFLICT DO NOTHING",
            )
            .bind(platform)
            .bind(mid)
            .bind(rule_id)
            .bind(conversation_id)
            .bind(customer_id)
            .bind(now_iso())
            .bind(now_iso())
            .execute(db)
            .await;
            match inserted {
                Ok(r) if r.rows_affected() == 1 => Reservation::Reserved,
                Ok(_) => Reservation::Pending, // concurrent insert won
                Err(e) => Reservation::Error(e.to_string()),
            }
        }
    }
}

/// Follow/greeting evaluation (CRD 1424-1428): highest-priority welcome rule
/// only; no duplicate-ledger consultation.
pub async fn evaluate_welcome(
    state: &AppState,
    platform: &str,
    team_id: Option<i64>,
    conversation_id: &str,
    customer_id: i64,
    platform_user_id: &str,
    reply_token: Option<&str>,
) -> EvalResult {
    let rules = match applicable_rules(state, team_id).await {
        Ok(r) => r,
        Err(e) => {
            return EvalResult {
                matched: false,
                rule_id: None,
                sent: false,
                error: Some(e.to_string()),
            }
        }
    };
    let Some(rule) = rules
        .iter()
        .find(|r| r.trigger_type == "welcome" || r.trigger_type == "greeting")
    else {
        return EvalResult::default();
    };
    dispatch(
        state,
        rule,
        &DispatchContext {
            platform,
            conversation_id,
            team_id,
            customer_id,
            platform_user_id,
            reply_token,
            trigger_content: "[follow]",
            matched_condition: json!({"trigger": "welcome", "event": "follow"}),
        },
    )
    .await
}

/// Redelivery recovery (CRD 1430-1433): re-run message evaluation for an
/// already-stored duplicate; the ledger guard keeps it at-most-once.
pub async fn retry_redelivered(
    state: &AppState,
    platform: &str,
    platform_message_id: &str,
    platform_user_id: &str,
    reply_token: Option<&str>,
) -> EvalResult {
    let original: Option<OriginalMessageRow> = sqlx::query_as(
        "SELECT m.content, m.content_type, m.conversation_id, c.team_id, m.customer_id
         FROM messages m JOIN conversations c ON c.id = m.conversation_id
         WHERE m.platform_message_id = $1",
    )
    .bind(platform_message_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let Some((content, content_type, conversation_id, team_id, customer_id)) = original else {
        return EvalResult {
            matched: false,
            rule_id: None,
            sent: false,
            error: Some("original message not found for redelivered identifier".into()),
        };
    };
    evaluate_message(
        state,
        MessageEvalInput {
            platform,
            content: &content,
            message_type: &content_type,
            conversation_id: &conversation_id,
            team_id,
            customer_id: customer_id.unwrap_or_default(),
            platform_user_id,
            platform_message_id: Some(platform_message_id),
            reply_token,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{apply_delivery_update_outcome, DeliveryUpdateOutcome, EvalResult};

    #[test]
    fn delivery_update_failure_marks_successful_result_with_error() {
        let result = apply_delivery_update_outcome(
            EvalResult {
                matched: true,
                rule_id: Some(42),
                sent: true,
                error: None,
            },
            "line",
            "mid-1",
            DeliveryUpdateOutcome::Failed("database unavailable".to_string()),
        );

        assert!(result.sent, "outbound delivery already happened");
        assert_eq!(result.rule_id, Some(42));
        assert_eq!(
            result.error.as_deref(),
            Some("auto-reply delivery ledger update failed: database unavailable")
        );
    }

    #[test]
    fn delivery_update_failure_preserves_delivery_error() {
        let result = apply_delivery_update_outcome(
            EvalResult {
                matched: true,
                rule_id: Some(7),
                sent: false,
                error: Some("channel rejected send".to_string()),
            },
            "line",
            "mid-2",
            DeliveryUpdateOutcome::Failed("database unavailable".to_string()),
        );

        assert_eq!(result.error.as_deref(), Some("channel rejected send"));
    }
}
