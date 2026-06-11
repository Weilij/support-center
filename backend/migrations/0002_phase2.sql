-- Phase 2 — Teams (§3.2) & Agents/Operators (§3.3) additions.
-- Teams gain an optional unique join-QR value and a cached QR image (CRD 1838, 2092).
-- Agents gain a per-operator skill inventory and a presence system (CRD 2301-2307).

ALTER TABLE teams ADD COLUMN qr_code TEXT;
ALTER TABLE teams ADD COLUMN qr_code_image TEXT;
-- QR-code value uniqueness IS enforced when provided (CRD 1844).
CREATE UNIQUE INDEX idx_teams_qr_code ON teams(qr_code) WHERE qr_code IS NOT NULL;

CREATE TABLE agent_skills (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    category TEXT NOT NULL,                -- communication|technical|product|language|platform|soft_skill
    level TEXT NOT NULL,                   -- beginner|intermediate|advanced|expert
    description TEXT,
    certified INTEGER NOT NULL DEFAULT 0,
    certified_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT,
    UNIQUE(agent_id, name)                 -- skill names are unique per operator (CRD 2221)
);

CREATE TABLE agent_status (
    agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'offline', -- online|busy|away|offline|break|meeting
    since TEXT NOT NULL,
    available_until TEXT,
    note TEXT,
    updated_at TEXT
);

CREATE TABLE agent_status_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    since TEXT NOT NULL,
    available_until TEXT,
    note TEXT,
    recorded_at TEXT NOT NULL
);
CREATE INDEX idx_agent_status_history_agent ON agent_status_history(agent_id, id);
