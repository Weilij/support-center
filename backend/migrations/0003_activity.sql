-- Activity Log & Reversible Actions (CRD §3.5, lines 2448-2612).
-- Restore bookkeeping columns: the guarded one-time transition that makes a
-- restore exactly-once observable as not-restored / in-progress / restored.
ALTER TABLE activity_logs ADD COLUMN restore_state TEXT;          -- NULL | 'in_progress' | 'restored'
ALTER TABLE activity_logs ADD COLUMN restored_by_log_id BIGINT;  -- restore audit entry id (link)
ALTER TABLE activity_logs ADD COLUMN restored_at TEXT;

CREATE INDEX idx_activity_logs_created_at ON activity_logs(created_at);
CREATE INDEX idx_activity_logs_action ON activity_logs(action);
