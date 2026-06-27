use serde_json::Value;
use std::time::Instant;

use super::{
    default_preferences, RealtimeHub, UserState, ACCESS_CACHE_TTL, MAX_SUBSCRIPTIONS_PER_USER,
};

impl RealtimeHub {
    /// Subscribe a user's personal channel to a conversation (CRD 3413).
    /// Returns the new subscription count, or `None` at the per-account ceiling.
    pub fn subscribe(&self, user_id: &str, conversation_id: &str) -> Option<usize> {
        let mut inner = self.inner.lock();
        let user = inner.users.entry(user_id.to_string()).or_default();
        if !user.subscriptions.contains(conversation_id)
            && user.subscriptions.len() >= MAX_SUBSCRIPTIONS_PER_USER
        {
            return None;
        }
        if user.subscriptions.insert(conversation_id.to_string()) {
            user.conversations_joined += 1;
        }
        Some(user.subscriptions.len())
    }

    /// Unsubscribe always succeeds (CRD 3413). Returns the remaining count.
    pub fn unsubscribe(&self, user_id: &str, conversation_id: &str) -> usize {
        let mut inner = self.inner.lock();
        let user = inner.users.entry(user_id.to_string()).or_default();
        user.subscriptions.remove(conversation_id);
        user.subscriptions.len()
    }

    pub fn is_subscribed(&self, user_id: &str, conversation_id: &str) -> bool {
        let inner = self.inner.lock();
        inner
            .users
            .get(user_id)
            .is_some_and(|u| u.subscriptions.contains(conversation_id))
    }

    pub fn note_message_sent(&self, user_id: &str) {
        self.inner
            .lock()
            .users
            .entry(user_id.to_string())
            .or_default()
            .messages_sent += 1;
    }

    pub fn note_messages_received(&self, user_id: &str, count: u64) {
        self.inner
            .lock()
            .users
            .entry(user_id.to_string())
            .or_default()
            .messages_received += count;
    }

    /// Whether the user's realtime state is currently held in memory.
    pub fn has_user_state(&self, user_id: &str) -> bool {
        self.inner.lock().users.contains_key(user_id)
    }

    /// Restore a persisted user-state snapshot (CRD 3812-3815). A no-op when
    /// in-memory state already exists (live sessions are authoritative).
    pub fn hydrate_user(
        &self,
        user_id: &str,
        last_seen: Option<String>,
        subscriptions: Vec<String>,
        preferences: Option<Value>,
        stats: Option<&Value>,
    ) {
        let mut inner = self.inner.lock();
        if inner.users.contains_key(user_id) {
            return;
        }
        let stat = |key: &str| {
            stats
                .and_then(|s| s.get(key))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        inner.users.insert(
            user_id.to_string(),
            UserState {
                subscriptions: subscriptions.into_iter().collect(),
                online: false,
                last_seen,
                total_sessions: stat("totalSessions"),
                messages_sent: stat("messagesSent"),
                messages_received: stat("messagesReceived"),
                conversations_joined: stat("conversationsJoined"),
                preferences,
            },
        );
    }

    /// Consolidated per-user state snapshot (CRD 3765, 3815): identity, online
    /// flag, last-seen, live-session count, followed conversations, preferences
    /// and activity statistics.
    pub fn user_state_snapshot(&self, user_id: &str) -> Value {
        let inner = self.inner.lock();
        let sessions = inner
            .conns
            .values()
            .filter(|c| c.identity.user_id == user_id)
            .count();
        match inner.users.get(user_id) {
            Some(u) => Self::user_snapshot_json(user_id, u, sessions),
            None => Self::user_snapshot_json(user_id, &UserState::default(), sessions),
        }
    }

    /// Presence heartbeat (CRD 3743-3748): marks the user online and refreshes
    /// last-seen. Returns (online, lastSeen).
    pub fn heartbeat(&self, user_id: &str) -> (bool, String) {
        let mut inner = self.inner.lock();
        let user = inner.users.entry(user_id.to_string()).or_default();
        user.online = true;
        let now = crate::db::now_iso();
        user.last_seen = Some(now.clone());
        (true, now)
    }

    /// Current notification preferences (defaults when never set, CRD 3813).
    pub fn preferences(&self, user_id: &str) -> Value {
        let inner = self.inner.lock();
        inner
            .users
            .get(user_id)
            .and_then(|u| u.preferences.clone())
            .unwrap_or_else(default_preferences)
    }

    /// Shallow-merge supplied preference fields over the current preferences
    /// (CRD 3755-3759); returns the merged result.
    pub fn merge_preferences(&self, user_id: &str, patch: &Value) -> Value {
        let mut inner = self.inner.lock();
        let user = inner.users.entry(user_id.to_string()).or_default();
        let mut current = user.preferences.clone().unwrap_or_else(default_preferences);
        if let (Some(cur), Some(new)) = (current.as_object_mut(), patch.as_object()) {
            for (k, v) in new {
                cur.insert(k.clone(), v.clone());
            }
        }
        user.preferences = Some(current.clone());
        current
    }

    pub fn subscription_count(&self, user_id: &str) -> usize {
        let inner = self.inner.lock();
        inner
            .users
            .get(user_id)
            .map(|u| u.subscriptions.len())
            .unwrap_or(0)
    }

    /// Cached agent->conversation access decision (~5 minutes, CRD 3258).
    pub fn cached_access(&self, user_id: &str, conversation_id: &str) -> Option<bool> {
        let inner = self.inner.lock();
        inner
            .access_cache
            .get(&(user_id.to_string(), conversation_id.to_string()))
            .filter(|(at, _)| at.elapsed() < ACCESS_CACHE_TTL)
            .map(|(_, allowed)| *allowed)
    }

    pub fn cache_access(&self, user_id: &str, conversation_id: &str, allowed: bool) {
        let mut inner = self.inner.lock();
        inner
            .access_cache
            .retain(|_, (at, _)| at.elapsed() < ACCESS_CACHE_TTL);
        inner.access_cache.insert(
            (user_id.to_string(), conversation_id.to_string()),
            (Instant::now(), allowed),
        );
    }

    /// Invalidate cached access for a conversation when assignments change
    /// (CRD 3258, 646).
    pub fn invalidate_access(&self, conversation_id: &str) {
        self.inner
            .lock()
            .access_cache
            .retain(|(_, cid), _| cid != conversation_id);
    }
}
