-- Cross-instance customer-channel fan-out. Each backend process keeps its own
-- in-memory WebSocket registry, so raw customer-channel events are durably
-- relayed through Postgres and acknowledged once per receiving instance.

CREATE TABLE realtime_customer_fanout_events (
    id TEXT PRIMARY KEY,
    source_instance TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    event TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_realtime_customer_fanout_pending
    ON realtime_customer_fanout_events(created_at);
CREATE INDEX idx_realtime_customer_fanout_conversation
    ON realtime_customer_fanout_events(conversation_id);

CREATE TABLE realtime_customer_fanout_acks (
    event_id TEXT NOT NULL REFERENCES realtime_customer_fanout_events(id) ON DELETE CASCADE,
    instance_id TEXT NOT NULL,
    acked_at TEXT NOT NULL,
    PRIMARY KEY (event_id, instance_id)
);
