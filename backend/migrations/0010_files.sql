-- Phase 5: File & Attachment Management (CRD §4.4, lines 2996-3216).
-- The attachments table (0001) gains the richer file-record fields.

ALTER TABLE attachments ADD COLUMN original_name TEXT;
ALTER TABLE attachments ADD COLUMN platform TEXT DEFAULT 'system';
ALTER TABLE attachments ADD COLUMN file_type TEXT;          -- image|video|audio|document|archive|other
ALTER TABLE attachments ADD COLUMN public_url TEXT;
ALTER TABLE attachments ADD COLUMN updated_at TEXT;

CREATE INDEX idx_attachments_uploader ON attachments(uploaded_by, created_at);
CREATE INDEX idx_attachments_conversation_files ON attachments(conversation_id, created_at);
