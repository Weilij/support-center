-- Phase 6: Notifications (CRD §6.4, lines 4881-5104).

ALTER TABLE notifications ADD COLUMN priority TEXT NOT NULL DEFAULT 'normal';
ALTER TABLE notifications ADD COLUMN updated_at TEXT;

CREATE INDEX idx_notifications_unread ON notifications(agent_id, is_read, created_at);

-- Monitoring alert records (CRD 5083): retained ~30 days.
CREATE TABLE monitoring_alerts (
    id TEXT PRIMARY KEY,
    level TEXT NOT NULL,                      -- info|warning|critical|emergency
    title TEXT NOT NULL,
    description TEXT,
    acknowledged BIGINT NOT NULL DEFAULT 0,
    acknowledged_by TEXT,
    acknowledged_at TEXT,
    resolved BIGINT NOT NULL DEFAULT 0,
    resolved_at TEXT,
    channel_attempts TEXT,                    -- JSON array of {channel, time, success, error}
    metadata TEXT,
    created_at TEXT NOT NULL
);
