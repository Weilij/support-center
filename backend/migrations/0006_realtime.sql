-- Realtime gateway persistence (CRD §5.1): feature/migration configuration,
-- error/quality analytics records, operational alerts, alert thresholds.

CREATE TABLE realtime_config (
    id BIGINT PRIMARY KEY CHECK (id = 1),
    config TEXT NOT NULL,
    updated_by TEXT,
    updated_at TEXT
);

-- Analytics: record error (CRD 3366-3371).
CREATE TABLE realtime_error_events (
    id TEXT PRIMARY KEY,
    timestamp TEXT NOT NULL,
    error_code TEXT NOT NULL,
    error_type TEXT NOT NULL,
    details TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_realtime_errors_ts ON realtime_error_events(timestamp);

-- Analytics: record connection quality (CRD 3373-3377).
CREATE TABLE realtime_quality_samples (
    id TEXT PRIMARY KEY,
    timestamp TEXT NOT NULL,
    user_id TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    details TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_realtime_quality_ts ON realtime_quality_samples(timestamp);

-- Operational alerts (CRD 3350-3352, 3379-3383).
CREATE TABLE realtime_alerts (
    id TEXT PRIMARY KEY,
    level TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    triggered_by TEXT,
    resolved BIGINT NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

-- Alert thresholds (CRD 3389-3397); a missing row means defaults are in effect.
CREATE TABLE realtime_alert_config (
    id BIGINT PRIMARY KEY CHECK (id = 1),
    error_rate_threshold DOUBLE PRECISION NOT NULL,
    latency_threshold DOUBLE PRECISION NOT NULL,
    connection_failure_threshold DOUBLE PRECISION NOT NULL,
    satisfaction_threshold DOUBLE PRECISION NOT NULL,
    time_window BIGINT NOT NULL,
    updated_at TEXT
);
