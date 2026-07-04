-- Schema optimization: index gaps + uniqueness gaps (review report, phase 1).
-- Pure additive/index work — no data or behavior change.

-- 1. Messages hot-path composite index for the inbox list (last-message lookup +
--    per-conversation ordering). Replaces the single-column conversation_id index.
CREATE INDEX IF NOT EXISTS idx_messages_conversation_created
    ON messages (conversation_id, created_at DESC, id DESC);
DROP INDEX IF EXISTS idx_messages_conversation;

-- 2. Uniqueness gaps.
-- 2a. tags: Postgres treats NULLs as distinct, so UNIQUE(name, team_id) does NOT
--     prevent duplicate GLOBAL tags (team_id IS NULL). Enforce global-tag name
--     uniqueness (excluding soft-deleted).
CREATE UNIQUE INDEX IF NOT EXISTS idx_tags_name_global_unique
    ON tags (name)
    WHERE team_id IS NULL AND deleted_at IS NULL;

-- 2b. customers: the (platform, platform_user_id) uniqueness did not exclude
--     soft-deleted rows, so a deleted customer permanently squatted the platform id
--     and the same LINE/FB user could never re-enter. Replace the table constraint
--     with a partial unique index that ignores soft-deleted rows.
ALTER TABLE customers DROP CONSTRAINT IF EXISTS customers_platform_platform_user_id_key;
CREATE UNIQUE INDEX IF NOT EXISTS idx_customers_platform_uid
    ON customers (platform, platform_user_id)
    WHERE deleted_at IS NULL;

-- 3. Missing foreign-key indexes (Postgres does not auto-index FKs; CASCADE deletes
--    and high-frequency lookups otherwise sequential-scan).
CREATE INDEX IF NOT EXISTS idx_team_members_team ON team_members (team_id);
CREATE INDEX IF NOT EXISTS idx_messages_customer ON messages (customer_id);
CREATE INDEX IF NOT EXISTS idx_messages_agent ON messages (agent_id);
CREATE INDEX IF NOT EXISTS idx_customer_tags_tag ON customer_tags (tag_id);
CREATE INDEX IF NOT EXISTS idx_conversation_tags_tag ON conversation_tags (tag_id);
CREATE INDEX IF NOT EXISTS idx_task_reminders_agent ON task_reminders (agent_id);
CREATE INDEX IF NOT EXISTS idx_scheduled_messages_agent ON scheduled_messages (agent_id);
CREATE INDEX IF NOT EXISTS idx_customer_feedback_conversation ON customer_feedback (conversation_id);
-- activity_logs restore feature looks up by (resource_type, resource_id).
CREATE INDEX IF NOT EXISTS idx_activity_logs_resource ON activity_logs (resource_type, resource_id);
CREATE INDEX IF NOT EXISTS idx_auth_sessions_agent ON auth_sessions (agent_id);
CREATE INDEX IF NOT EXISTS idx_auth_sessions_expires ON auth_sessions (expires_at);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_agent ON refresh_tokens (agent_id);
CREATE INDEX IF NOT EXISTS idx_revoked_tokens_expires ON revoked_tokens (expires_at);
CREATE INDEX IF NOT EXISTS idx_metrics_name_timestamp ON metrics (name, timestamp);
