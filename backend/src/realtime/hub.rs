//! Central realtime hub (CRD §5.1): connection registry, per-conversation rooms,
//! per-user personal channels, team/global broadcast groups, and the broadcast
//! API consumed by the domain modules.
//!
//! Every live connection in this process is registered here with an mpsc sender;
//! local fan-out is a synchronous walk over matching connections. Cross-instance
//! delivery is handled by `realtime::broadcaster` fan-out rows and domain
//! helpers that mirror local events to peer instances.

use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;
use tokio::sync::mpsc;

mod room_state;
mod snapshots;
mod user_state;

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

/// Authentication challenge lifetime (CRD 3515: 30 seconds).
pub const CHALLENGE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Single-use room authentication challenge (CRD 3511-3518, 3666).
#[derive(Clone)]
pub struct Challenge {
    pub user_id: String,
    pub role: String,
    pub display_name: String,
    /// The originating credential the signature is keyed against (CRD 3666).
    pub token: String,
    pub token_exp: i64,
    expires: Instant,
}

struct RoomState {
    /// "full" | "simplified" — set at room creation (CRD 3479).
    mode: String,
    /// Monotonically increasing in-room message order counter (CRD 3559).
    seq: u64,
    /// Bounded recent-message history used for reconnection sync (CRD 3562).
    history: VecDeque<Value>,
    last_message_at: Option<String>,
    last_activity: String,
    created: Instant,
    /// Outstanding single-use authentication challenges (full mode, CRD 3665).
    challenges: HashMap<String, Challenge>,
}

impl Default for RoomState {
    fn default() -> Self {
        Self {
            mode: "full".into(),
            seq: 0,
            history: VecDeque::new(),
            last_message_at: None,
            last_activity: crate::db::now_iso(),
            created: Instant::now(),
            challenges: HashMap::new(),
        }
    }
}

#[derive(Default)]
struct UserState {
    subscriptions: HashSet<String>,
    /// Presence flag (CRD 3820): live sessions imply online; a heartbeat also
    /// marks the user online explicitly (CRD 3825).
    online: bool,
    last_seen: Option<String>,
    total_sessions: u64,
    messages_sent: u64,
    messages_received: u64,
    conversations_joined: u64,
    preferences: Option<Value>,
}

impl UserState {
    fn stats_json(&self) -> Value {
        json!({
            "totalSessions": self.total_sessions,
            "messagesSent": self.messages_sent,
            "messagesReceived": self.messages_received,
            "conversationsJoined": self.conversations_joined,
        })
    }
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
    pub presence_transition: Option<PresenceTransition>,
}

#[derive(Clone, Debug)]
pub struct PresenceTransition {
    pub identity: ConnIdentity,
    pub status: &'static str,
}

pub struct UnregisterOutcome {
    pub snapshot: Value,
    pub presence_transition: PresenceTransition,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// Per-account or global connection ceiling reached -> 429 (CRD 3256).
    CeilingReached(&'static str),
}

pub struct RealtimeHub {
    instance_id: String,
    inner: Mutex<HubInner>,
    config: Mutex<GatewayConfig>,
    /// Routed-delivery queues, reachability registry & distribution statistics
    /// (CRD §5.2 lines 3581-3660).
    pub queue: super::broadcaster::BroadcastQueue,
    /// Per-conversation customer-side channels (CRD §5.4 lines 3847-3974).
    pub customers: super::customer::CustomerChannels,
    /// Realtime-module runtime config, event statistics, alerts & metrics
    /// history (CRD §5.5 lines 3974-4197).
    pub module: super::module::ModuleState,
    /// Latest-message cache (CRD §5.5 lines 4129-4166).
    pub latest: super::latest::LatestMessageCache,
    /// Collaboration presence/typing/viewer state (CRD §3.4 lines 2321-2446).
    pub collab: super::collaboration::CollabState,
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
            instance_id: uuid::Uuid::new_v4().to_string(),
            inner: Mutex::new(HubInner::default()),
            config: Mutex::new(GatewayConfig::default()),
            queue: super::broadcaster::BroadcastQueue::default(),
            customers: super::customer::CustomerChannels::default(),
            module: super::module::ModuleState::default(),
            latest: super::latest::LatestMessageCache::default(),
            collab: super::collaboration::CollabState::default(),
            started: Instant::now(),
        }
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    // ------------------------------------------------------------- configuration

    pub fn config(&self) -> GatewayConfig {
        self.config.lock().clone()
    }

    pub fn set_config(&self, new: GatewayConfig) {
        *self.config.lock() = new;
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
        let mut inner = self.inner.lock();
        let user_conns = inner
            .conns
            .values()
            .filter(|c| c.identity.user_id == identity.user_id)
            .count();
        if user_conns >= MAX_CONNECTIONS_PER_USER {
            return Err(RegisterError::CeilingReached(
                "per-user connection limit reached",
            ));
        }
        if inner.conns.len() >= MAX_CONNECTIONS_GLOBAL {
            return Err(RegisterError::CeilingReached(
                "global connection limit reached",
            ));
        }
        if let Some(cid) = &conversation_id {
            let room_conns = inner
                .conns
                .values()
                .filter(|c| c.conversation_id.as_deref() == Some(cid))
                .count();
            if room_conns >= ROOM_CAPACITY {
                return Err(RegisterError::CeilingReached(
                    "room connection limit reached",
                ));
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
        user.online = true;
        user.last_seen = Some(crate::db::now_iso());
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
                // The welcome's last-message timestamp reflects stored history
                // in full mode and is null in simplified mode (CRD 3498).
                let (mode, last_message_at) = inner
                    .rooms
                    .get(cid)
                    .map(|r| {
                        let last = if r.mode == "simplified" {
                            None
                        } else {
                            r.last_message_at.clone()
                        };
                        (r.mode.clone(), last)
                    })
                    .unwrap_or(("full".into(), None));
                // Welcome event to the new socket only (CRD 3441, 3683).
                let welcome = frame(
                    "connection_established",
                    json!({
                        "conversationId": cid,
                        "connectionId": connection_id,
                        "participants": participants,
                        "roomMode": mode,
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
                        u.stats_json(),
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

        Ok(Registration {
            connection_id,
            rx,
            presence_transition: first_overall.then_some(PresenceTransition {
                identity,
                status: "online",
            }),
        })
    }

    /// Remove a connection (socket close, error, forced disconnect, reap).
    /// When this was the user's last live connection the user's final state
    /// snapshot (offline, last-seen refreshed) is returned so the caller can
    /// re-persist it (CRD 3828).
    pub fn unregister(&self, connection_id: &str) -> Option<UnregisterOutcome> {
        let mut inner = self.inner.lock();
        let entry = inner.conns.remove(connection_id)?;
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

        // Last connection overall => offline + state evicted from memory after
        // taking a final snapshot for persistence (CRD 3428, 3431, 3824, 3828).
        let user_still_connected = inner
            .conns
            .values()
            .any(|c| c.identity.user_id == identity.user_id);
        if !user_still_connected {
            let snapshot = inner.users.remove(&identity.user_id).map(|mut u| {
                u.online = false;
                u.last_seen = Some(crate::db::now_iso());
                Self::user_snapshot_json(&identity.user_id, &u, 0)
            });
            Self::presence_locked(&inner, &identity, "offline");
            return snapshot.map(|snapshot| UnregisterOutcome {
                snapshot,
                presence_transition: PresenceTransition {
                    identity,
                    status: "offline",
                },
            });
        }
        None
    }

    /// Forced removal by the disconnect endpoints (CRD 3270-3278, 3524-3528).
    /// Only the owning account (or an administrator) may remove a connection;
    /// anything else is a no-op. Returns the final user-state snapshot when
    /// the user's last connection was removed (for persistence).
    pub fn remove_connection(
        &self,
        connection_id: &str,
        caller: &str,
        caller_is_admin: bool,
    ) -> Option<UnregisterOutcome> {
        let owned = {
            let inner = self.inner.lock();
            match inner.conns.get(connection_id) {
                Some(c) => caller_is_admin || c.identity.user_id == caller,
                None => return None,
            }
        };
        if !owned {
            return None;
        }
        self.unregister(connection_id)
    }

    /// Refresh a connection's (and its room's) last-activity timestamp
    /// (CRD 3545).
    pub fn touch(&self, connection_id: &str) {
        let mut inner = self.inner.lock();
        let room = match inner.conns.get_mut(connection_id) {
            Some(c) => {
                c.last_activity = Instant::now();
                c.conversation_id.clone()
            }
            None => None,
        };
        if let Some(cid) = room {
            if let Some(r) = inner.rooms.get_mut(&cid) {
                r.last_activity = crate::db::now_iso();
            }
        }
    }

    /// Connections idle past the inactivity timeout are reaped (CRD 3431).
    /// Returns the ids so callers can close their sockets.
    pub fn idle_connections(&self) -> Vec<String> {
        let inner = self.inner.lock();
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
        let event = if status == "online" {
            "user_connected"
        } else {
            "user_disconnected"
        };
        let payload = json!({
            "userId": identity.user_id,
            "userName": identity.display_name,
            "status": status,
        });
        let f = frame(event, payload);
        let mut sent: HashSet<&str> = HashSet::new();
        for c in inner.conns.values() {
            let is_admin = c.identity.role == "admin";
            let in_team = c
                .identity
                .team_ids
                .iter()
                .any(|t| identity.team_ids.contains(t));
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
        let inner = self.inner.lock();
        let identity = ConnIdentity {
            user_id: user_id.to_string(),
            email: String::new(),
            display_name: display_name.to_string(),
            role: "agent".into(),
            team_ids: team_ids.to_vec(),
        };
        let f = frame(
            "presence_changed",
            json!({
                "userId": user_id,
                "userName": display_name,
                "status": status,
            }),
        );
        for c in inner.conns.values() {
            let is_admin = c.identity.role == "admin";
            let in_team = c
                .identity
                .team_ids
                .iter()
                .any(|t| identity.team_ids.contains(t));
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
        let mut inner = self.inner.lock();
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
            let inner = self.inner.lock();
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

    /// Conversation audience, raw frame: delivers an already-serialized JSON
    /// frame verbatim (no `{type,payload,timestamp}` wrapper) to the room's
    /// connections and to personal channels subscribed to the conversation.
    /// Used where the CRD pins the exact top-level frame shape (e.g. the
    /// latest-message refresh notification, CRD 4180-4182).
    pub fn to_conversation_raw(&self, conversation_id: &str, raw: &str) -> usize {
        if !self.broadcasts_enabled() {
            return 0;
        }
        let inner = self.inner.lock();
        let subscribed: HashSet<&String> = inner
            .users
            .iter()
            .filter(|(_, u)| u.subscriptions.contains(conversation_id))
            .map(|(id, _)| id)
            .collect();
        let mut delivered = 0usize;
        for c in inner.conns.values() {
            let matches = c.conversation_id.as_deref() == Some(conversation_id)
                || (c.conversation_id.is_none() && subscribed.contains(&c.identity.user_id));
            if matches && c.tx.send(raw.to_string()).is_ok() {
                delivered += 1;
            }
        }
        delivered
    }

    /// Conversation audience minus one excluded user — typing indicators and
    /// collaboration events never echo to their originator (CRD 4093, 2439).
    pub fn to_conversation_except_user(
        &self,
        conversation_id: &str,
        exclude_user: &str,
        event: &str,
        payload: Value,
    ) -> usize {
        if !self.broadcasts_enabled() {
            return 0;
        }
        let subscribed: HashSet<String> = {
            let inner = self.inner.lock();
            inner
                .users
                .iter()
                .filter(|(_, u)| u.subscriptions.contains(conversation_id))
                .map(|(id, _)| id.clone())
                .collect()
        };
        self.fan_out(
            |c| {
                c.identity.user_id != exclude_user
                    && (c.conversation_id.as_deref() == Some(conversation_id)
                        || (c.conversation_id.is_none()
                            && subscribed.contains(&c.identity.user_id)))
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
            let mut inner = self.inner.lock();
            let room = inner.rooms.entry(conversation_id.to_string()).or_default();
            room.last_message_at = Some(crate::db::now_iso());
            room.last_activity = crate::db::now_iso();
            // Simplified rooms keep no recent-message history (CRD 3562, 3571).
            if room.mode != "simplified" {
                room.history.push_back(payload.clone());
                while room.history.len() > ROOM_HISTORY_CAP {
                    room.history.pop_front();
                }
            }
        }
        // Cross-instance callers mirror this event through
        // `broadcaster::publish_remote_event`; the hub only owns local room
        // state and local connection fan-out.
        self.to_conversation(conversation_id, event, payload)
    }

    /// Deliver one already-framed message to a single connection (protocol
    /// replies: pong, acks, error frames).
    pub fn to_connection(&self, connection_id: &str, framed: String) -> bool {
        let inner = self.inner.lock();
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
        let mut inner = self.inner.lock();
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
        let inner = self.inner.lock();
        let Some(room) = inner.rooms.get(conversation_id) else {
            return (Vec::new(), None);
        };
        let missed = match since {
            None => Vec::new(),
            Some(since) => room
                .history
                .iter()
                .filter(|m| {
                    m.get("timestamp")
                        .and_then(Value::as_str)
                        .is_some_and(|t| t > since)
                })
                .cloned()
                .collect(),
        };
        (missed, room.last_message_at.clone())
    }

    fn user_snapshot_json(user_id: &str, user: &UserState, session_count: usize) -> Value {
        json!({
            "userId": user_id,
            "online": user.online,
            "lastSeen": user.last_seen,
            "sessionCount": session_count,
            "subscriptions": user.subscriptions.iter().cloned().collect::<Vec<_>>(),
            "preferences": user.preferences.clone().unwrap_or_else(default_preferences),
            "stats": user.stats_json(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_lock_survives_panic_while_held() {
        let hub = RealtimeHub::new();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = hub.inner.lock();
            panic!("simulate panic while holding realtime hub lock");
        }));

        assert!(result.is_err());
        assert_eq!(hub.connection_count(), 0);
    }
}
