-- Cross-instance routed broadcaster fan-out. Queued broadcaster events are
-- persisted once, delivered by peer backend instances into their local hub, and
-- acknowledged once per receiving instance.

CREATE TABLE realtime_broadcast_fanout_events (
    id TEXT PRIMARY KEY,
    source_instance TEXT NOT NULL,
    event TEXT NOT NULL,
    targets TEXT NOT NULL,
    options TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_realtime_broadcast_fanout_pending
    ON realtime_broadcast_fanout_events(created_at);

CREATE TABLE realtime_broadcast_fanout_acks (
    event_id TEXT NOT NULL REFERENCES realtime_broadcast_fanout_events(id) ON DELETE CASCADE,
    instance_id TEXT NOT NULL,
    acked_at TEXT NOT NULL,
    PRIMARY KEY (event_id, instance_id)
);
