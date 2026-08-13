-- Agent-facing text for a failed outbound delivery, paired with `reject_code`
-- (0022). Kept in its own column rather than inside `messages.metadata`:
-- metadata is client-supplied JSON of any shape, so writing into it needs a
-- jsonb cast that raises on non-object values, and the customer-facing history
-- serializes metadata verbatim.
ALTER TABLE messages ADD COLUMN reject_message TEXT;
