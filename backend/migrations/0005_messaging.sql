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
    delivered INTEGER NOT NULL DEFAULT 0,
    delivered_at TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    expires_at TEXT NOT NULL
);
CREATE INDEX idx_offline_buffer_recipient ON offline_message_buffer(recipient_id, delivered);
CREATE INDEX idx_offline_buffer_expiry ON offline_message_buffer(expires_at);

-- Recall-log entries may target delayed (scheduled) items as well as persisted
-- messages (CRD 999, 1016), so the hard FK to messages(id) must go. SQLite
-- requires a rebuild to drop a foreign key.
CREATE TABLE message_recall_logs_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    action TEXT,                                  -- 'successful' | 'failed'
    created_at TEXT NOT NULL
);
INSERT INTO message_recall_logs_new (id, message_id, agent_id, action, created_at)
    SELECT id, message_id, agent_id, action, created_at FROM message_recall_logs;
DROP TABLE message_recall_logs;
ALTER TABLE message_recall_logs_new RENAME TO message_recall_logs;
CREATE INDEX idx_message_recall_logs_message ON message_recall_logs(message_id);

CREATE INDEX idx_scheduled_messages_status ON scheduled_messages(status, scheduled_at);
