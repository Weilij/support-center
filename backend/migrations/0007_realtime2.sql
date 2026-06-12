-- User real-time session persistence (CRD §5.3 lines 3812-3815): the
-- consolidated per-user state snapshot — followed conversations, notification
-- preferences and activity statistics — persisted on relevant changes and
-- restored when the user's realtime state is next needed (session open or
-- HTTP state access), so it survives full disconnects and restarts.

CREATE TABLE realtime_user_state (
    user_id TEXT PRIMARY KEY,
    online BIGINT NOT NULL DEFAULT 0,
    last_seen TEXT,
    subscriptions TEXT NOT NULL DEFAULT '[]',
    preferences TEXT,
    stats TEXT,
    updated_at TEXT
);
