-- Messaging (CRD §2.2, lines 830-1042) additions.

-- Offline-buffer entries (CRD 1018, 1038): per-recipient buffered copies of
-- real-time messages for offline users, with retention, retry, and delivery
-- bookkeeping.
CREATE TABLE offline_message_buffer (
    id TEXT PRIMARY KEY,
    message_id TEXT,
    recipient_id TEXT NOT NULL,
    conversation_id TEXT,
    payload TEXT,                                 -- JSON message payload
    buffered_at TEXT NOT NULL,
    delivered BIGINT NOT NULL DEFAULT 0,
    delivered_at TEXT,
    retry_count BIGINT NOT NULL DEFAULT 0,
    expires_at TEXT NOT NULL
);
CREATE INDEX idx_offline_buffer_recipient ON offline_message_buffer(recipient_id, delivered);
CREATE INDEX idx_offline_buffer_expiry ON offline_message_buffer(expires_at);

-- Recall-log entries may target delayed (scheduled) items as well as persisted
-- messages (CRD 999, 1016), so the hard FK to messages(id) must go.
ALTER TABLE message_recall_logs DROP CONSTRAINT message_recall_logs_message_id_fkey;
ALTER TABLE message_recall_logs DROP CONSTRAINT message_recall_logs_agent_id_fkey;
ALTER TABLE message_recall_logs ALTER COLUMN agent_id SET NOT NULL;
CREATE INDEX idx_message_recall_logs_message ON message_recall_logs(message_id);

CREATE INDEX idx_scheduled_messages_status ON scheduled_messages(status, scheduled_at);
