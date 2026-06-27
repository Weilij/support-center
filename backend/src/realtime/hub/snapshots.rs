use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

use super::RealtimeHub;

impl RealtimeHub {
    pub fn connection_count(&self) -> usize {
        self.inner.lock().conns.len()
    }

    /// Number of live sessions held by one user.
    pub fn user_session_count(&self, user_id: &str) -> usize {
        let inner = self.inner.lock();
        inner
            .conns
            .values()
            .filter(|c| c.identity.user_id == user_id)
            .count()
    }

    /// Live reachability derived from current connections: (user ids with a
    /// personal channel, conversation ids with a live room connection). Used
    /// by the routed-delivery debug snapshot (CRD 3656-3657, 3669).
    pub fn reachability_snapshot(&self) -> (Vec<String>, Vec<String>) {
        let inner = self.inner.lock();
        let mut users = HashSet::new();
        let mut conversations = HashSet::new();
        for c in inner.conns.values() {
            match &c.conversation_id {
                Some(cid) => {
                    conversations.insert(cid.clone());
                }
                None => {
                    users.insert(c.identity.user_id.clone());
                }
            }
        }
        (
            users.into_iter().collect(),
            conversations.into_iter().collect(),
        )
    }

    /// (total, conversation-bound, personal) connection counts.
    pub fn connection_breakdown(&self) -> (usize, usize, usize) {
        let inner = self.inner.lock();
        let total = inner.conns.len();
        let rooms = inner
            .conns
            .values()
            .filter(|c| c.conversation_id.is_some())
            .count();
        (total, rooms, total - rooms)
    }

    /// Broadcast error rate derived from per-send delivery counters (CRD 3296).
    pub fn error_rate(&self) -> f64 {
        let inner = self.inner.lock();
        let total = inner.broadcasts_delivered + inner.send_failures;
        if total == 0 {
            return 0.0;
        }
        inner.send_failures as f64 / total as f64
    }

    /// (attempted, delivered, failed) broadcast counters for metrics views.
    pub fn broadcast_counters(&self) -> (u64, u64, u64) {
        let inner = self.inner.lock();
        (
            inner.broadcasts_attempted,
            inner.broadcasts_delivered,
            inner.send_failures,
        )
    }

    /// Currently tracked connections for the operational dashboard (CRD 3332).
    pub fn connections_snapshot(&self) -> Vec<Value> {
        let inner = self.inner.lock();
        inner
            .conns
            .values()
            .map(|c| {
                json!({
                    "connectionId": c.id,
                    "userId": c.identity.user_id,
                    "role": c.identity.role,
                    "conversationId": c.conversation_id,
                    "deviceId": c.device_id,
                    "connectedAt": c.connected_at,
                    "active": true,
                })
            })
            .collect()
    }

    /// Connection counts by conversation, by account and by protocol (CRD 3329).
    pub fn dashboard_counts(&self) -> Value {
        let inner = self.inner.lock();
        let mut by_conversation: HashMap<String, usize> = HashMap::new();
        let mut by_user: HashMap<String, usize> = HashMap::new();
        for c in inner.conns.values() {
            if let Some(cid) = &c.conversation_id {
                *by_conversation.entry(cid.clone()).or_default() += 1;
            }
            *by_user.entry(c.identity.user_id.clone()).or_default() += 1;
        }
        json!({
            "byConversation": by_conversation,
            "byUser": by_user,
            "byProtocol": { "websocket": inner.conns.len() },
        })
    }

    /// Per-user/conversation component snapshot for the connectivity self-test
    /// (CRD 3404-3408).
    pub fn test_snapshot(&self, user_id: &str, conversation_id: Option<&str>) -> Value {
        let inner = self.inner.lock();
        let user_conns = inner
            .conns
            .values()
            .filter(|c| c.identity.user_id == user_id)
            .count();
        let user_online = user_conns > 0;
        let room = conversation_id.map(|cid| {
            let conns = inner
                .conns
                .values()
                .filter(|c| c.conversation_id.as_deref() == Some(cid))
                .count();
            let participants: HashSet<&str> = inner
                .conns
                .values()
                .filter(|c| c.conversation_id.as_deref() == Some(cid))
                .map(|c| c.identity.user_id.as_str())
                .collect();
            json!({
                "conversationId": cid,
                "activeConnections": conns,
                "participantCount": participants.len(),
                "lastMessageAt": inner.rooms.get(cid).and_then(|r| r.last_message_at.clone()),
            })
        });
        json!({
            "userChannel": {
                "userId": user_id,
                "online": user_online,
                "activeConnections": user_conns,
                "subscriptions": inner
                    .users
                    .get(user_id)
                    .map(|u| u.subscriptions.iter().cloned().collect::<Vec<_>>())
                    .unwrap_or_default(),
            },
            "conversationRoom": room,
        })
    }
}
