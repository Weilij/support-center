//! Central realtime hub (CRD §5.1): connection registry, per-conversation rooms,
//! per-user personal channels, team/global broadcast groups, and the broadcast
//! API consumed by the domain modules.
//!
//! Single-process implementation: every live connection is registered here with
//! an mpsc sender; fan-out is a synchronous walk over the matching connections.
//! TODO(scale-out): multi-instance delivery (cross-instance room propagation,
//! shared reachability registry, fire-and-forget remote fan-out per CRD 3542,
//! 3467) is deferred — observable behavior is single-instance equivalent.

use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::mpsc;

/// Per-account live-connection ceiling (CRD 3719: cap is 5).
pub const MAX_CONNECTIONS_PER_USER: usize = 5;
/// Global connection ceiling enforced at handshake (CRD 3241, 3256).
pub const MAX_CONNECTIONS_GLOBAL: usize = 10_000;
/// Per-room connection capacity (CRD 3491: default cap 100).
pub const ROOM_CAPACITY: usize = 100;
/// Per-account conversation-subscription ceiling (CRD 3427: about 50).
pub const MAX_SUBSCRIPTIONS_PER_USER: usize = 50;
/// Bounded recent-message history per room (CRD 3562: default 50 entries).
pub const ROOM_HISTORY_CAP: usize = 50;
/// Inbound frame ceiling per connection (CRD 3419: ~10 frames per second).
pub const INBOUND_FRAMES_PER_SEC: usize = 10;
/// Inbound frame size ceiling (CRD 3419: about 10 KB).
pub const MAX_INBOUND_FRAME_BYTES: usize = 10_240;
/// Accessible-conversation authorization cache lifetime (CRD 3258: ~5 minutes).
pub const ACCESS_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);
/// Idle-connection reap threshold (CRD 3431: around 5 minutes).
pub const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Verified identity attached to a live connection (CRD 608).
#[derive(Clone, Debug)]
pub struct ConnIdentity {
    pub user_id: String,
    pub email: String,
    pub display_name: String,
    /// "admin" | "agent" — the only roles admitted by the gate (CRD 3238).
    pub role: String,
    pub team_ids: Vec<i64>,
}

struct ConnEntry {
    id: String,
    identity: ConnIdentity,
    /// `Some` = conversation-room connection; `None` = personal channel.
    conversation_id: Option<String>,
    device_id: Option<String>,
    connected_at: String,
    last_activity: Instant,
    tx: mpsc::UnboundedSender<String>,
}

#[derive(Default)]
struct RoomState {
    /// Monotonically increasing in-room message order counter (CRD 3559).
    seq: u64,
    /// Bounded recent-message history used for reconnection sync (CRD 3562).
    history: VecDeque<Value>,
    last_message_at: Option<String>,
}

#[derive(Default)]
struct UserState {
    subscriptions: HashSet<String>,
    total_sessions: u64,
    messages_sent: u64,
    conversations_joined: u64,
    preferences: Option<Value>,
}

fn default_preferences() -> Value {
    // CRD 3813: independent boolean toggles, all enabled by default.
    json!({
        "notificationSettings": {
            "newMessage": true,
            "messageRecall": true,
            "conversationAssignment": true,
            "systemNotifications": true,
        }
    })
}

#[derive(Default)]
struct HubInner {
    conns: HashMap<String, ConnEntry>,
    rooms: HashMap<String, RoomState>,
    users: HashMap<String, UserState>,
    /// (user, conversation) -> (checked-at, allowed); ~5-minute TTL (CRD 3258).
    access_cache: HashMap<(String, String), (Instant, bool)>,
    broadcasts_attempted: u64,
    broadcasts_delivered: u64,
    send_failures: u64,
}

/// Gateway feature/migration configuration (CRD 3280-3292).
#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub enabled: bool,
    pub strategy: String,
    pub rollout_percentage: i64,
    pub feature_flags: Map<String, Value>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        let mut flags = Map::new();
        flags.insert("realtimeMessaging".into(), json!(true));
        flags.insert("presenceTracking".into(), json!(true));
        flags.insert("typingIndicators".into(), json!(true));
        Self {
            enabled: true,
            strategy: "immediate".into(),
            rollout_percentage: 100,
            feature_flags: flags,
        }
    }
}

impl GatewayConfig {
    pub fn to_json(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "strategy": self.strategy,
            "rolloutPercentage": self.rollout_percentage,
            "featureFlags": Value::Object(self.feature_flags.clone()),
        })
    }
}

pub struct Registration {
    pub connection_id: String,
    pub rx: mpsc::UnboundedReceiver<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// Per-account or global connection ceiling reached -> 429 (CRD 3256).
    CeilingReached(&'static str),
}

pub struct RealtimeHub {
    inner: Mutex<HubInner>,
    config: Mutex<GatewayConfig>,
    started: Instant,
}

impl Default for RealtimeHub {
    fn default() -> Self {
        Self::new()
    }
}

/// Build one outbound event frame: `{ type, payload, timestamp }`.
pub fn frame(event: &str, payload: Value) -> String {
    json!({ "type": event, "payload": payload, "timestamp": crate::db::now_iso() }).to_string()
}

impl RealtimeHub {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HubInner::default()),
            config: Mutex::new(GatewayConfig::default()),
            started: Instant::now(),
        }
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    // ------------------------------------------------------------- configuration

    pub fn config(&self) -> GatewayConfig {
        self.config.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn set_config(&self, new: GatewayConfig) {
        if let Ok(mut c) = self.config.lock() {
            *c = new;
        }
    }

    /// Broadcasts are suppressed when the feature is disabled or the
    /// realtime-messaging flag is off (CRD 3467).
    pub fn broadcasts_enabled(&self) -> bool {
        let c = self.config();
        c.enabled
            && c.feature_flags
                .get("realtimeMessaging")
                .and_then(Value::as_bool)
                .unwrap_or(true)
    }

    // ------------------------------------------------- connection lifecycle

    /// Register a live connection. Enforces per-account / global / per-room
    /// ceilings, queues the welcome event, emits join/presence events.
    pub fn register(
        &self,
        identity: ConnIdentity,
        conversation_id: Option<String>,
        device_id: Option<String>,
    ) -> Result<Registration, RegisterError> {
        let mut inner = self.inner.lock().expect("hub lock");
        let user_conns =
            inner.conns.values().filter(|c| c.identity.user_id == identity.user_id).count();
        if user_conns >= MAX_CONNECTIONS_PER_USER {
            return Err(RegisterError::CeilingReached("per-user connection limit reached"));
        }
        if inner.conns.len() >= MAX_CONNECTIONS_GLOBAL {
            return Err(RegisterError::CeilingReached("global connection limit reached"));
        }
        if let Some(cid) = &conversation_id {
            let room_conns =
                inner.conns.values().filter(|c| c.conversation_id.as_deref() == Some(cid)).count();
            if room_conns >= ROOM_CAPACITY {
                return Err(RegisterError::CeilingReached("room connection limit reached"));
            }
        }

        let connection_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::unbounded_channel();
        let first_overall = user_conns == 0;
        let entry = ConnEntry {
            id: connection_id.clone(),
            identity: identity.clone(),
            conversation_id: conversation_id.clone(),
            device_id,
            connected_at: crate::db::now_iso(),
            last_activity: Instant::now(),
            tx,
        };
        inner.conns.insert(connection_id.clone(), entry);

        let user = inner.users.entry(identity.user_id.clone()).or_default();
        user.total_sessions += 1;
        if user.preferences.is_none() {
            user.preferences = Some(default_preferences());
        }

        match &conversation_id {
            Some(cid) => {
                inner.rooms.entry(cid.clone()).or_default();
                let participants: Vec<String> = {
                    let mut seen = HashSet::new();
                    inner
                        .conns
                        .values()
                        .filter(|c| c.conversation_id.as_deref() == Some(cid))
                        .filter(|c| seen.insert(c.identity.user_id.clone()))
                        .map(|c| c.identity.user_id.clone())
                        .collect()
                };
                let last_message_at =
                    inner.rooms.get(cid).and_then(|r| r.last_message_at.clone());
                // Welcome event to the new socket only (CRD 3441, 3683).
                let welcome = frame(
                    "connection_established",
                    json!({
                        "conversationId": cid,
                        "connectionId": connection_id,
                        "participants": participants,
                        "roomMode": "full",
                        "lastMessageAt": last_message_at,
                    }),
                );
                if let Some(c) = inner.conns.get(&connection_id) {
                    let _ = c.tx.send(welcome);
                }
                // user_joined to all room connections, joiner included (CRD 3497, 3684).
                let count = participants.len();
                let joined = frame(
                    "user_joined",
                    json!({
                        "userId": identity.user_id,
                        "connectionId": connection_id,
                        "role": identity.role,
                        "participantCount": count,
                    }),
                );
                for c in inner.conns.values() {
                    if c.conversation_id.as_deref() == Some(cid.as_str()) {
                        let _ = c.tx.send(joined.clone());
                    }
                }
            }
            None => {
                let (subs, prefs, stats) = {
                    let u = inner.users.get(&identity.user_id).expect("user state");
                    (
                        u.subscriptions.iter().cloned().collect::<Vec<_>>(),
                        u.preferences.clone().unwrap_or_else(default_preferences),
                        json!({
                            "totalSessions": u.total_sessions,
                            "messagesSent": u.messages_sent,
                            "conversationsJoined": u.conversations_joined,
                        }),
                    )
                };
                // Personal-channel welcome (CRD 3442, 3833).
                let welcome = frame(
                    "user_connected",
                    json!({
                        "userId": identity.user_id,
                        "connectionId": connection_id,
                        "subscriptions": subs,
                        "preferences": prefs,
                        "stats": stats,
                    }),
                );
                if let Some(c) = inner.conns.get(&connection_id) {
                    let _ = c.tx.send(welcome);
                }
            }
        }

        // Presence: first connection => online, broadcast to the account's
        // team(s) and to administrators (CRD 3432, 3446).
        if first_overall {
            Self::presence_locked(&inner, &identity, "online");
        }

        Ok(Registration { connection_id, rx })
    }

    /// Remove a connection (socket close, error, forced disconnect, reap).
    pub fn unregister(&self, connection_id: &str) {
        let mut inner = self.inner.lock().expect("hub lock");
        let Some(entry) = inner.conns.remove(connection_id) else { return };
        let identity = entry.identity.clone();

        // user_left fires once per user departure from the room (CRD 3577).
        if let Some(cid) = &entry.conversation_id {
            let user_still_in_room = inner.conns.values().any(|c| {
                c.conversation_id.as_deref() == Some(cid.as_str())
                    && c.identity.user_id == identity.user_id
            });
            if !user_still_in_room {
                let count = {
                    let mut seen = HashSet::new();
                    inner
                        .conns
                        .values()
                        .filter(|c| c.conversation_id.as_deref() == Some(cid.as_str()))
                        .filter(|c| seen.insert(c.identity.user_id.clone()))
                        .count()
                };
                let left = frame(
                    "user_left",
                    json!({
                        "userId": identity.user_id,
                        "connectionId": connection_id,
                        "participantCount": count,
                    }),
                );
                for c in inner.conns.values() {
                    if c.conversation_id.as_deref() == Some(cid.as_str()) {
                        let _ = c.tx.send(left.clone());
                    }
                }
            }
        }

        // Last connection overall => offline + state removed (CRD 3428, 3431).
        let user_still_connected =
            inner.conns.values().any(|c| c.identity.user_id == identity.user_id);
        if !user_still_connected {
            inner.users.remove(&identity.user_id);
            Self::presence_locked(&inner, &identity, "offline");
        }
    }

    /// Forced removal by the disconnect endpoint (CRD 3270-3278). Only the
    /// owning account (or an administrator) may remove a connection.
    pub fn remove_connection(&self, connection_id: &str, caller: &str, caller_is_admin: bool) -> bool {
        let owned = {
            let inner = self.inner.lock().expect("hub lock");
            match inner.conns.get(connection_id) {
                Some(c) => caller_is_admin || c.identity.user_id == caller,
                None => return false,
            }
        };
        if !owned {
            return false;
        }
        self.unregister(connection_id);
        true
    }

    /// Refresh a connection's last-activity timestamp (CRD 3545).
    pub fn touch(&self, connection_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(c) = inner.conns.get_mut(connection_id) {
                c.last_activity = Instant::now();
            }
        }
    }

    /// Connections idle past the inactivity timeout are reaped (CRD 3431).
    /// Returns the ids so callers can close their sockets.
    pub fn idle_connections(&self) -> Vec<String> {
        let inner = self.inner.lock().expect("hub lock");
        inner
            .conns
            .values()
            .filter(|c| c.last_activity.elapsed() > IDLE_TIMEOUT)
            .map(|c| c.id.clone())
            .collect()
    }

    fn presence_locked(inner: &HubInner, identity: &ConnIdentity, status: &str) {
        // Presence events go to the account's team(s) and administrators
        // (CRD 3446); event names follow the taxonomy's user connected /
        // user disconnected identifiers (CRD 3438).
        let event = if status == "online" { "user_connected" } else { "user_disconnected" };
        let payload = json!({
            "userId": identity.user_id,
            "userName": identity.display_name,
            "status": status,
        });
        let f = frame(event, payload);
        let mut sent: HashSet<&str> = HashSet::new();
        for c in inner.conns.values() {
            let is_admin = c.identity.role == "admin";
            let in_team = c.identity.team_ids.iter().any(|t| identity.team_ids.contains(t));
            if (is_admin || in_team)
                && c.identity.user_id != identity.user_id
                && sent.insert(c.id.as_str())
            {
                let _ = c.tx.send(f.clone());
            }
        }
    }

    /// Presence status change (online / offline / away / available / busy),
    /// broadcast to the account's team(s) and administrators (CRD 3446).
    pub fn presence(&self, user_id: &str, display_name: &str, status: &str, team_ids: &[i64]) {
        if !self.broadcasts_enabled() {
            return;
        }
        let inner = self.inner.lock().expect("hub lock");
        let identity = ConnIdentity {
            user_id: user_id.to_string(),
            email: String::new(),
            display_name: display_name.to_string(),
            role: "agent".into(),
            team_ids: team_ids.to_vec(),
        };
        let f = frame("presence_changed", json!({
            "userId": user_id,
            "userName": display_name,
            "status": status,
        }));
        for c in inner.conns.values() {
            let is_admin = c.identity.role == "admin";
            let in_team = c.identity.team_ids.iter().any(|t| identity.team_ids.contains(t));
            if (is_admin || in_team) && c.identity.user_id != identity.user_id {
                let _ = c.tx.send(f.clone());
            }
        }
    }

    // ------------------------------------------------------- broadcast API

    fn fan_out<F: Fn(&ConnEntry) -> bool>(&self, pred: F, event: &str, payload: Value) -> usize {
        if !self.broadcasts_enabled() {
            return 0;
        }
        let mut inner = self.inner.lock().expect("hub lock");
        inner.broadcasts_attempted += 1;
        let f = frame(event, payload);
        let mut delivered: usize = 0;
        let mut failed: u64 = 0;
        for c in inner.conns.values() {
            if pred(c) {
                if c.tx.send(f.clone()).is_ok() {
                    delivered += 1;
                } else {
                    failed += 1;
                }
            }
        }
        inner.broadcasts_delivered += delivered as u64;
        inner.send_failures += failed;
        delivered
    }

    /// Conversation audience: room connections plus personal channels
    /// subscribed to the conversation (CRD 3464).
    pub fn to_conversation(&self, conversation_id: &str, event: &str, payload: Value) -> usize {
        if !self.broadcasts_enabled() {
            return 0;
        }
        let subscribed: HashSet<String> = {
            let inner = self.inner.lock().expect("hub lock");
            inner
                .users
                .iter()
                .filter(|(_, u)| u.subscriptions.contains(conversation_id))
                .map(|(id, _)| id.clone())
                .collect()
        };
        self.fan_out(
            |c| {
                c.conversation_id.as_deref() == Some(conversation_id)
                    || (c.conversation_id.is_none() && subscribed.contains(&c.identity.user_id))
            },
            event,
            payload,
        )
    }

    /// Conversation message fan-out: also advances the room's last-message
    /// timestamp and bounded history used for reconnection sync (CRD 3562).
    pub fn to_conversation_message(
        &self,
        conversation_id: &str,
        event: &str,
        payload: Value,
    ) -> usize {
        {
            let mut inner = self.inner.lock().expect("hub lock");
            let room = inner.rooms.entry(conversation_id.to_string()).or_default();
            room.last_message_at = Some(crate::db::now_iso());
            room.history.push_back(payload.clone());
            while room.history.len() > ROOM_HISTORY_CAP {
                room.history.pop_front();
            }
        }
        self.to_conversation(conversation_id, event, payload)
    }

    /// Deliver one already-framed message to a single connection (protocol
    /// replies: pong, acks, error frames).
    pub fn to_connection(&self, connection_id: &str, framed: String) -> bool {
        let inner = self.inner.lock().expect("hub lock");
        inner
            .conns
            .get(connection_id)
            .map(|c| c.tx.send(framed).is_ok())
            .unwrap_or(false)
    }

    /// Specific-account audience: every live session of one user (CRD 3464).
    pub fn to_user(&self, user_id: &str, event: &str, payload: Value) -> usize {
        self.fan_out(|c| c.identity.user_id == user_id, event, payload)
    }

    pub fn to_team(&self, team_id: i64, event: &str, payload: Value) -> usize {
        self.fan_out(|c| c.identity.team_ids.contains(&team_id), event, payload)
    }

    /// Team audience plus administrators (CRD 3464: "optionally including
    /// administrators alongside a team").
    pub fn to_teams_and_admins(&self, team_ids: &[i64], event: &str, payload: Value) -> usize {
        self.fan_out(
            |c| {
                c.identity.role == "admin"
                    || c.identity.team_ids.iter().any(|t| team_ids.contains(t))
            },
            event,
            payload,
        )
    }

    pub fn to_admins(&self, event: &str, payload: Value) -> usize {
        self.fan_out(|c| c.identity.role == "admin", event, payload)
    }

    /// Global fan-out, optionally filtered by role (CRD 3464).
    pub fn global(&self, event: &str, payload: Value) -> usize {
        self.fan_out(|_| true, event, payload)
    }

    pub fn global_role(&self, role: &str, event: &str, payload: Value) -> usize {
        self.fan_out(|c| c.identity.role == role, event, payload)
    }

    // ------------------------------------------------- room chat / protocol

    /// Next value of the room's monotonically increasing order counter.
    pub fn next_seq(&self, conversation_id: &str) -> u64 {
        let mut inner = self.inner.lock().expect("hub lock");
        let room = inner.rooms.entry(conversation_id.to_string()).or_default();
        room.seq += 1;
        room.seq
    }

    /// Relay a frame to the other connections of a room (typing indicators,
    /// never echoed to the sender; CRD 3687).
    pub fn relay_to_room_others(
        &self,
        conversation_id: &str,
        sender_connection: &str,
        event: &str,
        payload: Value,
    ) -> usize {
        self.fan_out(
            |c| c.conversation_id.as_deref() == Some(conversation_id) && c.id != sender_connection,
            event,
            payload,
        )
    }

    /// Reconnection sync (CRD 3416, 3569-3572): messages newer than `since`.
    pub fn sync_since(
        &self,
        conversation_id: &str,
        since: Option<&str>,
    ) -> (Vec<Value>, Option<String>) {
        let inner = self.inner.lock().expect("hub lock");
        let Some(room) = inner.rooms.get(conversation_id) else {
            return (Vec::new(), None);
        };
        let missed = match since {
            None => Vec::new(),
            Some(since) => room
                .history
                .iter()
                .filter(|m| {
                    m.get("timestamp").and_then(Value::as_str).is_some_and(|t| t > since)
                })
                .cloned()
                .collect(),
        };
        (missed, room.last_message_at.clone())
    }

    // ------------------------------------------------------- subscriptions

    /// Subscribe a user's personal channel to a conversation (CRD 3413).
    /// Returns the new subscription count, or `None` at the per-account ceiling.
    pub fn subscribe(&self, user_id: &str, conversation_id: &str) -> Option<usize> {
        let mut inner = self.inner.lock().expect("hub lock");
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
        let mut inner = self.inner.lock().expect("hub lock");
        let user = inner.users.entry(user_id.to_string()).or_default();
        user.subscriptions.remove(conversation_id);
        user.subscriptions.len()
    }

    pub fn is_subscribed(&self, user_id: &str, conversation_id: &str) -> bool {
        let inner = self.inner.lock().expect("hub lock");
        inner
            .users
            .get(user_id)
            .is_some_and(|u| u.subscriptions.contains(conversation_id))
    }

    pub fn note_message_sent(&self, user_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.users.entry(user_id.to_string()).or_default().messages_sent += 1;
        }
    }

    // --------------------------------------------------- authorization cache

    /// Cached agent->conversation access decision (~5 minutes, CRD 3258).
    pub fn cached_access(&self, user_id: &str, conversation_id: &str) -> Option<bool> {
        let inner = self.inner.lock().expect("hub lock");
        inner
            .access_cache
            .get(&(user_id.to_string(), conversation_id.to_string()))
            .filter(|(at, _)| at.elapsed() < ACCESS_CACHE_TTL)
            .map(|(_, allowed)| *allowed)
    }

    pub fn cache_access(&self, user_id: &str, conversation_id: &str, allowed: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.access_cache.retain(|_, (at, _)| at.elapsed() < ACCESS_CACHE_TTL);
            inner
                .access_cache
                .insert((user_id.to_string(), conversation_id.to_string()), (Instant::now(), allowed));
        }
    }

    /// Invalidate cached access for a conversation when assignments change
    /// (CRD 3258, 646).
    pub fn invalidate_access(&self, conversation_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.access_cache.retain(|(_, cid), _| cid != conversation_id);
        }
    }

    // ------------------------------------------------------------ snapshots

    pub fn connection_count(&self) -> usize {
        self.inner.lock().map(|i| i.conns.len()).unwrap_or(0)
    }

    /// (total, conversation-bound, personal) connection counts.
    pub fn connection_breakdown(&self) -> (usize, usize, usize) {
        let inner = self.inner.lock().expect("hub lock");
        let total = inner.conns.len();
        let rooms = inner.conns.values().filter(|c| c.conversation_id.is_some()).count();
        (total, rooms, total - rooms)
    }

    /// Broadcast error rate derived from per-send delivery counters (CRD 3296).
    pub fn error_rate(&self) -> f64 {
        let inner = self.inner.lock().expect("hub lock");
        let total = inner.broadcasts_delivered + inner.send_failures;
        if total == 0 {
            return 0.0;
        }
        inner.send_failures as f64 / total as f64
    }

    /// (attempted, delivered, failed) broadcast counters for metrics views.
    pub fn broadcast_counters(&self) -> (u64, u64, u64) {
        let inner = self.inner.lock().expect("hub lock");
        (inner.broadcasts_attempted, inner.broadcasts_delivered, inner.send_failures)
    }

    /// Currently tracked connections for the operational dashboard (CRD 3332).
    pub fn connections_snapshot(&self) -> Vec<Value> {
        let inner = self.inner.lock().expect("hub lock");
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
        let inner = self.inner.lock().expect("hub lock");
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
        let inner = self.inner.lock().expect("hub lock");
        let user_conns =
            inner.conns.values().filter(|c| c.identity.user_id == user_id).count();
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
