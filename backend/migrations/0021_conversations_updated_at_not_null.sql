-- Schema optimization phase 2: make the inbox sort key indexable.
-- The list orders by conversations.updated_at (was COALESCE(updated_at, created_at)),
-- which no plain index could serve. Backfill the nullable column, make it NOT NULL,
-- and index (updated_at DESC, id) to match the ORDER BY exactly.

UPDATE conversations SET updated_at = created_at WHERE updated_at IS NULL;

ALTER TABLE conversations ALTER COLUMN updated_at SET NOT NULL;

-- Replace the single-column updated_at index with the composite matching the sort.
DROP INDEX IF EXISTS idx_conversations_updated;
CREATE INDEX IF NOT EXISTS idx_conversations_updated_id
    ON conversations (updated_at DESC, id);
