//! Topic derivation and session-boundary detection (CRD 447-468, 480).

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::store::SessionRow;

/// Boundary configuration defaults (CRD 480).
pub const INACTIVITY_THRESHOLD_MINUTES: i64 = 30;
pub const MAX_MESSAGES_PER_SESSION: i64 = 50;
pub const MAX_DURATION_HOURS: i64 = 24;
pub const TOPIC_DETECTION_ENABLED: bool = true;

/// Cue phrases marking a customer-initiated topic change (CRD 480).
const TOPIC_CHANGE_CUES: &[&str] = &[
    "another question",
    "different question",
    "new question",
    "different topic",
    "change topic",
    "change the subject",
    "by the way",
    "one more thing",
    "unrelated question",
    "on another note",
];

/// Keyword-driven topic categories used by derivation and suggestions.
const TOPIC_CATEGORIES: &[(&str, &[&str])] = &[
    ("Billing & Payments", &["refund", "billing", "payment", "invoice", "charge", "charged"]),
    ("Technical Support", &["bug", "error", "crash", "broken", "not working", "issue", "problem"]),
    ("Orders & Shipping", &["order", "delivery", "shipping", "track", "package", "shipment"]),
    ("Account & Login", &["account", "password", "login", "sign in", "log in", "signin"]),
    ("Plans & Pricing", &["price", "cost", "plan", "subscription", "upgrade", "downgrade"]),
    ("General Inquiry", &["hello", "hi", "hey", "question", "help"]),
];

pub struct TopicResult {
    pub topic: String,
    pub confidence: f64,
    pub source: &'static str,
}

/// Derive a topic from message text: keyword categories first, then a truncated
/// excerpt of the message itself (CRD 345, 464-465).
pub fn derive_topic(content: &str) -> TopicResult {
    let lower = content.to_lowercase();
    for (topic, keywords) in TOPIC_CATEGORIES {
        if keywords.iter().any(|k| lower.contains(k)) {
            return TopicResult { topic: topic.to_string(), confidence: 0.8, source: "keyword" };
        }
    }
    let trimmed = content.trim();
    let topic = if trimmed.is_empty() {
        "General Inquiry".to_string()
    } else {
        trimmed.chars().take(50).collect()
    };
    TopicResult { topic, confidence: 0.3, source: "excerpt" }
}

/// Ranked topic suggestions for a message (CRD 467-468).
pub fn suggest_topics(content: &str, limit: usize) -> Vec<Value> {
    let lower = content.to_lowercase();
    let mut scored: Vec<(&str, usize)> = TOPIC_CATEGORIES
        .iter()
        .map(|(topic, keywords)| {
            (*topic, keywords.iter().filter(|k| lower.contains(*k)).count())
        })
        .filter(|(_, hits)| *hits > 0)
        .collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.1));
    let mut out: Vec<Value> = scored
        .iter()
        .take(limit)
        .map(|(topic, hits)| {
            json!({ "topic": topic, "confidence": (0.5 + 0.1 * (*hits as f64)).min(0.95) })
        })
        .collect();
    if out.len() < limit {
        let fallback = derive_topic(content);
        if !out.iter().any(|v| v["topic"] == json!(fallback.topic)) {
            out.push(json!({ "topic": fallback.topic, "confidence": fallback.confidence }));
        }
    }
    out.truncate(limit);
    out
}

pub struct Detection {
    pub should_create_new: bool,
    pub reason: &'static str,
    pub confidence: f64,
    pub suggested_topic: Option<String>,
    pub metadata: Value,
}

impl Detection {
    pub fn to_json(&self) -> Value {
        json!({
            "shouldCreateNew": self.should_create_new,
            "reason": self.reason,
            "confidence": self.confidence,
            "suggestedTopic": self.suggested_topic,
            "metadata": self.metadata,
        })
    }
}

fn parse_ts(raw: Option<&str>) -> Option<DateTime<Utc>> {
    raw.and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc))
}

/// Decide whether the incoming message extends the current active session or
/// starts a new segment. Reasons in priority order: first-session, time-gap,
/// message-limit, duration-limit, topic-change; otherwise continue (CRD 480).
pub fn detect_boundary(
    session: Option<&SessionRow>,
    message_content: &str,
    sender_type: &str,
    now: DateTime<Utc>,
) -> Detection {
    let thresholds = json!({
        "inactivityThresholdMinutes": INACTIVITY_THRESHOLD_MINUTES,
        "maxMessagesPerSession": MAX_MESSAGES_PER_SESSION,
        "maxDurationHours": MAX_DURATION_HOURS,
        "topicDetectionEnabled": TOPIC_DETECTION_ENABLED,
    });
    let suggested = || Some(derive_topic(message_content).topic);

    let Some(s) = session.filter(|s| s.is_active != 0) else {
        return Detection {
            should_create_new: true,
            reason: "first_session",
            confidence: 1.0,
            suggested_topic: suggested(),
            metadata: thresholds,
        };
    };

    if let Some(last) = parse_ts(s.last_activity_at.as_deref()) {
        let idle_minutes = (now - last).num_minutes();
        if idle_minutes > INACTIVITY_THRESHOLD_MINUTES {
            return Detection {
                should_create_new: true,
                reason: "time_gap",
                confidence: 0.9,
                suggested_topic: suggested(),
                metadata: json!({ "idleMinutes": idle_minutes, "thresholds": thresholds }),
            };
        }
    }
    if s.message_count >= MAX_MESSAGES_PER_SESSION {
        return Detection {
            should_create_new: true,
            reason: "message_limit",
            confidence: 0.8,
            suggested_topic: suggested(),
            metadata: json!({ "messageCount": s.message_count, "thresholds": thresholds }),
        };
    }
    if let Some(started) = parse_ts(s.started_at.as_deref()) {
        let age_hours = (now - started).num_hours();
        if age_hours > MAX_DURATION_HOURS {
            return Detection {
                should_create_new: true,
                reason: "duration_limit",
                confidence: 0.8,
                suggested_topic: suggested(),
                metadata: json!({ "ageHours": age_hours, "thresholds": thresholds }),
            };
        }
    }
    if TOPIC_DETECTION_ENABLED && sender_type == "customer" {
        let lower = message_content.to_lowercase();
        if let Some(cue) = TOPIC_CHANGE_CUES.iter().find(|c| lower.contains(*c)) {
            return Detection {
                should_create_new: true,
                reason: "topic_change",
                confidence: 0.7,
                suggested_topic: suggested(),
                metadata: json!({ "matchedCue": cue, "thresholds": thresholds }),
            };
        }
    }
    Detection {
        should_create_new: false,
        reason: "continue",
        confidence: 0.9,
        suggested_topic: None,
        metadata: thresholds,
    }
}
