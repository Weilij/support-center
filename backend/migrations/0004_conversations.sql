-- Conversations (§2.1) & Conversation-Session Management (§1.2B) additions.
-- Conversation sessions gain the documented optional attributes: priority,
-- sentiment, a tag list, and a metadata bag (CRD 473-474).

ALTER TABLE conversation_sessions ADD COLUMN priority TEXT;       -- low|medium|high|urgent
ALTER TABLE conversation_sessions ADD COLUMN sentiment TEXT;      -- positive|negative|neutral
ALTER TABLE conversation_sessions ADD COLUMN tags TEXT;           -- JSON array of strings
ALTER TABLE conversation_sessions ADD COLUMN metadata TEXT;       -- JSON object

CREATE INDEX idx_conversation_sessions_conversation
    ON conversation_sessions(conversation_id);
CREATE INDEX idx_messages_session ON messages(session_id);
CREATE INDEX idx_conversations_updated ON conversations(updated_at);
