-- Conceptual data model per Rust_CRD.md §7.2 (lines 5729-5832).
-- All timestamps are TEXT (ISO-8601 UTC strings). Soft-delete = nullable deleted_at.

CREATE TABLE teams (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT,
    onboarding_image TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    display_name TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'agent',          -- 'admin' | 'agent'
    is_active INTEGER NOT NULL DEFAULT 1,
    password_policy TEXT NOT NULL DEFAULT 'changeable', -- changeable|unchangeable|must_change
    last_active_at TEXT,
    last_login_at TEXT,
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT
);
-- email unique among ACTIVE (non-deleted) accounts only
CREATE UNIQUE INDEX idx_agents_email_active ON agents(email) WHERE deleted_at IS NULL;

CREATE TABLE team_members (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    team_id INTEGER NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member',         -- member|lead|supervisor
    is_primary INTEGER NOT NULL DEFAULT 0,
    joined_at TEXT NOT NULL,
    UNIQUE(agent_id, team_id)
);

CREATE TABLE customers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    platform TEXT NOT NULL,
    platform_user_id TEXT NOT NULL,
    display_name TEXT,
    avatar_url TEXT,
    email TEXT,
    phone TEXT,
    source_team_id INTEGER REFERENCES teams(id) ON DELETE SET NULL,
    metadata TEXT,
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT,
    UNIQUE(platform, platform_user_id)
);

CREATE TABLE qr_codes (
    id TEXT PRIMARY KEY,
    team_id INTEGER NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    token TEXT NOT NULL UNIQUE,
    url TEXT,
    image_url TEXT,
    campaign TEXT,
    description TEXT,
    scan_count INTEGER NOT NULL DEFAULT 0,
    max_scans INTEGER,
    is_active INTEGER NOT NULL DEFAULT 1,
    expires_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE qr_scans (
    id TEXT PRIMARY KEY,
    qr_code_id TEXT REFERENCES qr_codes(id) ON DELETE CASCADE,
    customer_id INTEGER REFERENCES customers(id) ON DELETE SET NULL,
    platform TEXT,
    platform_user_id TEXT,
    metadata TEXT,
    scanned_at TEXT NOT NULL
);

CREATE TABLE team_liff_links (
    id TEXT PRIMARY KEY,
    team_id INTEGER NOT NULL UNIQUE REFERENCES teams(id) ON DELETE CASCADE,
    url TEXT,
    image_url TEXT,
    scan_count INTEGER NOT NULL DEFAULT 0,
    is_active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE customer_team_assignments (
    id TEXT PRIMARY KEY,
    platform_user_id TEXT NOT NULL,
    team_id INTEGER NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    liff_link_id TEXT REFERENCES team_liff_links(id) ON DELETE SET NULL,
    source TEXT,                                  -- scan|manual|import|inbound
    display_name TEXT,
    assigned_at TEXT NOT NULL,
    metadata TEXT,
    UNIQUE(platform_user_id, team_id)
);

CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    customer_id INTEGER NOT NULL REFERENCES customers(id) ON DELETE RESTRICT,
    team_id INTEGER REFERENCES teams(id) ON DELETE SET NULL,
    status TEXT NOT NULL DEFAULT 'active',        -- active|assigned|pending|in-progress|waiting|closed
    priority TEXT NOT NULL DEFAULT 'normal',      -- low|normal|high|urgent
    first_response_at TEXT,
    closed_at TEXT,
    last_message_at TEXT,
    last_viewed_at TEXT,
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT
);
CREATE INDEX idx_conversations_team ON conversations(team_id);
CREATE INDEX idx_conversations_customer ON conversations(customer_id);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    sender_type TEXT NOT NULL,                    -- customer|agent|system
    customer_id INTEGER REFERENCES customers(id) ON DELETE SET NULL,
    agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL,
    content TEXT,
    content_type TEXT NOT NULL DEFAULT 'text',
    platform_message_id TEXT,
    is_recalled INTEGER NOT NULL DEFAULT 0,
    recall_deadline TEXT,
    recalled_at TEXT,
    is_sent INTEGER NOT NULL DEFAULT 1,
    sent_at TEXT,
    delivery_status TEXT NOT NULL DEFAULT 'delivered',
    reply_to_id TEXT,                             -- self-reference enforced by app logic only
    thread_id TEXT,
    session_id TEXT,
    session_seq INTEGER,
    metadata TEXT,
    sender_name TEXT,                             -- snapshot, survives account rename/removal
    read_by TEXT,                                 -- JSON array of agent ids
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT
);
CREATE INDEX idx_messages_conversation ON messages(conversation_id);

CREATE TABLE scheduled_messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    content TEXT,
    content_type TEXT NOT NULL DEFAULT 'text',
    scheduled_at TEXT,
    sent_at TEXT,
    cancelled_at TEXT,
    status TEXT NOT NULL DEFAULT 'pending',       -- pending|sent|cancelled|failed
    metadata TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE attachments (
    id TEXT PRIMARY KEY,
    message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
    file_name TEXT,
    content_type TEXT,
    file_size INTEGER,
    file_url TEXT,
    storage_key TEXT,
    upload_status TEXT NOT NULL DEFAULT 'completed',
    uploaded_by TEXT REFERENCES agents(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE conversation_sessions (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    session_type TEXT,
    topic TEXT,
    started_at TEXT,
    ended_at TEXT,
    last_activity_at TEXT,
    message_count INTEGER NOT NULL DEFAULT 0,
    is_active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE conversation_transfers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    from_team_id INTEGER REFERENCES teams(id) ON DELETE SET NULL,
    to_team_id INTEGER REFERENCES teams(id) ON DELETE SET NULL,
    reason TEXT,
    transferred_by TEXT NOT NULL REFERENCES agents(id) ON DELETE RESTRICT,
    transfer_type TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE message_recall_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE RESTRICT,
    action TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE notifications (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    type TEXT,
    title TEXT,
    content TEXT,
    data TEXT,
    is_read INTEGER NOT NULL DEFAULT 0,
    read_at TEXT,
    expires_at TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_notifications_agent ON notifications(agent_id);

CREATE TABLE tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    color TEXT NOT NULL DEFAULT '#3B82F6',
    description TEXT,
    team_id INTEGER REFERENCES teams(id) ON DELETE SET NULL,
    is_active INTEGER NOT NULL DEFAULT 1,
    created_by TEXT NOT NULL REFERENCES agents(id) ON DELETE RESTRICT,
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT,
    UNIQUE(name, team_id)
);

CREATE TABLE customer_tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    customer_id INTEGER NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    assigned_by TEXT REFERENCES agents(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    UNIQUE(customer_id, tag_id)
);

CREATE TABLE conversation_tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    assigned_by TEXT REFERENCES agents(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    UNIQUE(conversation_id, tag_id)
);

CREATE TABLE activity_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE RESTRICT,
    agent_name TEXT,
    agent_role TEXT,
    action TEXT NOT NULL,
    resource_type TEXT,
    resource_id TEXT,
    details TEXT,
    ip_address TEXT,
    user_agent TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_activity_logs_agent ON activity_logs(agent_id);

CREATE TABLE system_settings (
    key TEXT PRIMARY KEY,
    value TEXT,
    updated_at TEXT
);

CREATE TABLE metrics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    value REAL NOT NULL,
    timestamp INTEGER NOT NULL,
    tags TEXT,
    unit TEXT
);

CREATE TABLE channel_integrations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    team_id INTEGER NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    platform TEXT NOT NULL,
    config TEXT,
    credentials TEXT,                             -- protected/encrypted blob (or legacy plaintext)
    webhook_config TEXT,
    stats TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    is_verified INTEGER NOT NULL DEFAULT 0,
    verified_at TEXT,
    configured_by TEXT REFERENCES agents(id) ON DELETE SET NULL,
    metadata TEXT,
    last_error TEXT,
    error_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE webhook_security_events (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    severity TEXT NOT NULL,                       -- low|medium|high|critical
    platform TEXT,
    integration_id INTEGER REFERENCES channel_integrations(id) ON DELETE CASCADE,
    source_ip TEXT,
    details TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE cors_events (
    id TEXT PRIMARY KEY,
    outcome TEXT NOT NULL,                        -- allowed|rejected|preflight|streaming-connection|credentials-used
    origin TEXT,
    method TEXT,
    path TEXT,
    user_agent TEXT,
    ip_address TEXT,
    timestamp TEXT NOT NULL,
    metadata TEXT
);

CREATE TABLE reports (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    report_type TEXT,
    format TEXT,                                  -- json|excel|pdf|html|csv
    status TEXT NOT NULL DEFAULT 'pending',       -- pending|generating|completed|failed
    created_by TEXT NOT NULL REFERENCES agents(id) ON DELETE RESTRICT,
    team_id INTEGER REFERENCES teams(id) ON DELETE SET NULL,
    time_range TEXT,
    filters TEXT,
    generated_at TEXT,
    completed_at TEXT,
    failed_at TEXT,
    error_message TEXT,
    duration_ms INTEGER,
    output_url TEXT,
    output_size INTEGER,
    download_count INTEGER NOT NULL DEFAULT 0,
    last_downloaded_at TEXT,
    expires_at TEXT,
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE scheduled_reports (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    report_type TEXT,
    format TEXT,
    parameters TEXT,
    schedule_type TEXT,                           -- daily|weekly|monthly|custom
    schedule_config TEXT,
    timezone TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    max_retries INTEGER NOT NULL DEFAULT 3,
    retry_delay INTEGER,
    created_by TEXT NOT NULL REFERENCES agents(id) ON DELETE RESTRICT,
    team_id INTEGER REFERENCES teams(id) ON DELETE SET NULL,
    notify_on_success INTEGER NOT NULL DEFAULT 0,
    notify_on_failure INTEGER NOT NULL DEFAULT 1,
    recipients TEXT,
    next_run_at TEXT,
    last_run_at TEXT,
    last_status TEXT,
    run_count INTEGER NOT NULL DEFAULT 0,
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE scheduled_report_runs (
    id TEXT PRIMARY KEY,
    schedule_id TEXT NOT NULL REFERENCES scheduled_reports(id) ON DELETE CASCADE,
    started_at TEXT,
    completed_at TEXT,
    status TEXT,                                  -- running|success|failed|cancelled
    duration_ms INTEGER,
    report_id TEXT,
    error_message TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE report_downloads (
    id TEXT PRIMARY KEY,
    report_id TEXT NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
    downloaded_by TEXT NOT NULL REFERENCES agents(id) ON DELETE RESTRICT,
    downloaded_at TEXT NOT NULL,
    ip_address TEXT,
    user_agent TEXT,
    method TEXT,                                  -- manual|scheduled|programmatic
    size INTEGER
);

CREATE TABLE report_templates (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    report_type TEXT,
    config TEXT,
    preview_image TEXT,
    category TEXT,
    tags TEXT,
    is_system INTEGER NOT NULL DEFAULT 0,
    is_public INTEGER NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL REFERENCES agents(id) ON DELETE RESTRICT,
    team_id INTEGER REFERENCES teams(id) ON DELETE SET NULL,
    use_count INTEGER NOT NULL DEFAULT 0,
    last_used_at TEXT,
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE task_reminders (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    content TEXT,
    remind_at TEXT,
    conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
    repeat_type TEXT NOT NULL DEFAULT 'none',     -- none|daily|weekly|monthly
    repeat_interval INTEGER,
    is_completed INTEGER NOT NULL DEFAULT 0,
    is_sent INTEGER NOT NULL DEFAULT 0,
    completed_at TEXT,
    sent_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE customer_feedback (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    customer_id INTEGER NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
    agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL,
    rating INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 5),
    comment TEXT,
    feedback_type TEXT,                           -- satisfaction|service_quality|response_time
    metadata TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE auto_reply_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    team_id INTEGER REFERENCES teams(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    trigger_type TEXT NOT NULL,                   -- welcome|keyword|off_hours|fallback
    priority INTEGER NOT NULL DEFAULT 100,        -- lower = higher precedence
    is_active INTEGER NOT NULL DEFAULT 1,
    allow_fallback INTEGER NOT NULL DEFAULT 0,
    created_by TEXT REFERENCES agents(id) ON DELETE SET NULL,
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE auto_reply_conditions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    rule_id INTEGER NOT NULL REFERENCES auto_reply_rules(id) ON DELETE CASCADE,
    condition_type TEXT NOT NULL,                 -- exact|contains|pattern|message_type
    value TEXT,
    case_sensitive INTEGER NOT NULL DEFAULT 0,
    match_mode TEXT NOT NULL DEFAULT 'any'        -- any|all
);

CREATE TABLE auto_reply_actions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    rule_id INTEGER NOT NULL REFERENCES auto_reply_rules(id) ON DELETE CASCADE,
    action_type TEXT NOT NULL,                    -- text|image|rich
    content TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE auto_reply_business_hours (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    team_id INTEGER NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    day_of_week INTEGER NOT NULL,                 -- 0=Sunday .. 6=Saturday
    start_time TEXT,                              -- HH:MM
    end_time TEXT,                                -- HH:MM
    timezone TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    UNIQUE(team_id, day_of_week)
);

CREATE TABLE auto_reply_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    rule_id INTEGER REFERENCES auto_reply_rules(id) ON DELETE SET NULL,
    conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
    customer_id INTEGER REFERENCES customers(id) ON DELETE SET NULL,
    trigger_content TEXT,
    response_content TEXT,
    matched_condition TEXT,
    platform TEXT,
    delivery_method TEXT,                         -- reply|push
    created_at TEXT NOT NULL
);

CREATE TABLE auto_reply_deliveries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    platform TEXT NOT NULL,
    platform_message_id TEXT NOT NULL,
    rule_id INTEGER REFERENCES auto_reply_rules(id) ON DELETE SET NULL,
    conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
    customer_id INTEGER REFERENCES customers(id) ON DELETE SET NULL,
    status TEXT NOT NULL DEFAULT 'pending',       -- pending|success|failed
    delivery_method TEXT,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    last_attempt_at TEXT,
    sent_at TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(platform, platform_message_id)         -- at-most-once auto-reply per inbound message
);

-- Authentication infrastructure (§1.1 / §1.2A)

CREATE TABLE auth_sessions (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    data TEXT,                                    -- JSON profile snapshot
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT
);

CREATE TABLE refresh_tokens (
    jti TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    issued_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_at TEXT,
    revoked_at TEXT
);

CREATE TABLE revoked_tokens (
    jti TEXT PRIMARY KEY,
    agent_id TEXT,
    revoked_at TEXT NOT NULL,
    expires_at TEXT
);
