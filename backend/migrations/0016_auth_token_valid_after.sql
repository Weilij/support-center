-- Invalidate already-issued access/user/monitoring tokens after password changes.
ALTER TABLE agents ADD COLUMN tokens_valid_after TEXT;
