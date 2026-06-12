-- Phase 5 part 1: Channel Integrations (CRD §4.1) + Inbound Webhook Ingestion (CRD §4.2).

-- Dedup guarantee (CRD 2769, 2789): the originating platform message identifier is
-- effectively unique; a unique-constraint race on insert is caught by the ingest
-- pipeline and resolved to the existing record instead of erroring.
CREATE UNIQUE INDEX idx_messages_platform_message_id
    ON messages(platform_message_id)
    WHERE platform_message_id IS NOT NULL;

CREATE INDEX idx_channel_integrations_team
    ON channel_integrations(team_id, platform, is_active);
