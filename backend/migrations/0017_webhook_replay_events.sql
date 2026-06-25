-- Idempotency ledger for webhook lifecycle events that do not create messages.
CREATE TABLE IF NOT EXISTS webhook_replay_events (
    platform TEXT NOT NULL,
    event_type TEXT NOT NULL,
    event_key TEXT NOT NULL,
    seen_at TEXT NOT NULL,
    PRIMARY KEY (platform, event_type, event_key)
);
