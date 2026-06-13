-- Advisory position used by the frontend for feature gating. The backend stores
-- and returns it but makes no authorization decisions from it.
ALTER TABLE agents ADD COLUMN position TEXT;
