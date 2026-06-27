use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Instant;

use super::{Challenge, RealtimeHub, RoomState, CHALLENGE_TTL};

impl RealtimeHub {
    /// Create the room if absent, fixing its mode at creation time
    /// (CRD 3479: full vs simplified, set once). Returns the effective mode.
    pub fn ensure_room(&self, conversation_id: &str, mode: Option<&str>) -> String {
        let mut inner = self.inner.lock();
        let room = inner
            .rooms
            .entry(conversation_id.to_string())
            .or_insert_with(|| {
                let mut r = RoomState::default();
                if mode == Some("simplified") {
                    r.mode = "simplified".into();
                }
                r
            });
        room.mode.clone()
    }

    /// The room's mode; rooms default to full-featured (CRD 3479).
    pub fn room_mode(&self, conversation_id: &str) -> String {
        let inner = self.inner.lock();
        inner
            .rooms
            .get(conversation_id)
            .map(|r| r.mode.clone())
            .unwrap_or_else(|| "full".into())
    }

    fn room_participants_locked(
        inner: &super::HubInner,
        conversation_id: &str,
    ) -> (Vec<String>, usize) {
        let mut seen = HashSet::new();
        let mut conns = 0usize;
        let mut participants = Vec::new();
        for c in inner.conns.values() {
            if c.conversation_id.as_deref() == Some(conversation_id) {
                conns += 1;
                if seen.insert(c.identity.user_id.clone()) {
                    participants.push(c.identity.user_id.clone());
                }
            }
        }
        (participants, conns)
    }

    /// Participant listing for the room HTTP surface (CRD 3536-3537).
    pub fn room_info(&self, conversation_id: &str) -> Value {
        let inner = self.inner.lock();
        let (participants, conns) = Self::room_participants_locked(&inner, conversation_id);
        json!({
            "participants": participants,
            "activeConnections": conns,
            "lastActivity": inner.rooms.get(conversation_id).map(|r| r.last_activity.clone()),
        })
    }

    /// Room metrics (CRD 3539-3540): full mode additionally reports history
    /// length and an uptime estimate.
    pub fn room_metrics_snapshot(&self, conversation_id: &str) -> Value {
        let inner = self.inner.lock();
        let (participants, conns) = Self::room_participants_locked(&inner, conversation_id);
        let room = inner.rooms.get(conversation_id);
        let mode = room
            .map(|r| r.mode.clone())
            .unwrap_or_else(|| "full".into());
        let mut out = json!({
            "conversationId": conversation_id,
            "mode": mode,
            "activeConnections": conns,
            "participantCount": participants.len(),
            "messageSequence": room.map(|r| r.seq).unwrap_or(0),
            "lastActivity": room.map(|r| r.last_activity.clone()),
            "active": conns > 0,
        });
        if mode != "simplified" {
            out["historyLength"] = json!(room.map(|r| r.history.len()).unwrap_or(0));
            out["uptimeSeconds"] = json!(room.map(|r| r.created.elapsed().as_secs()).unwrap_or(0));
        }
        out
    }

    /// Deliver a fully-formed injected event to every active connection in
    /// the room as an event frame (CRD 3530-3534).
    pub fn room_broadcast_raw(&self, conversation_id: &str, event: Value) -> usize {
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("event")
            .to_string();
        self.fan_out(
            |c| c.conversation_id.as_deref() == Some(conversation_id),
            &event_type,
            event,
        )
    }

    /// Issue a single-use authentication challenge bound to the resolved
    /// identity and originating credential (CRD 3511-3518). Expired challenges
    /// are purged opportunistically.
    pub fn create_challenge(
        &self,
        conversation_id: &str,
        user_id: &str,
        role: &str,
        display_name: &str,
        token: &str,
        token_exp: i64,
    ) -> (String, String) {
        let mut inner = self.inner.lock();
        let room = inner.rooms.entry(conversation_id.to_string()).or_default();
        room.challenges.retain(|_, c| c.expires > Instant::now());
        let id = uuid::Uuid::new_v4().to_string();
        room.challenges.insert(
            id.clone(),
            Challenge {
                user_id: user_id.to_string(),
                role: role.to_string(),
                display_name: display_name.to_string(),
                token: token.to_string(),
                token_exp,
                expires: Instant::now() + CHALLENGE_TTL,
            },
        );
        let expires_at = (chrono::Utc::now() + chrono::Duration::from_std(CHALLENGE_TTL).unwrap())
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        (id, expires_at)
    }

    /// Consume a challenge: single-use, expired challenges are deleted on
    /// access and verify as absent (CRD 3518, 3676).
    pub fn consume_challenge(
        &self,
        conversation_id: &str,
        challenge_id: &str,
    ) -> Option<Challenge> {
        let mut inner = self.inner.lock();
        let room = inner.rooms.get_mut(conversation_id)?;
        let challenge = room.challenges.remove(challenge_id)?;
        (challenge.expires > Instant::now()).then_some(challenge)
    }
}
