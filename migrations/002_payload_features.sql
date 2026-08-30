CREATE TABLE callbacks (
    id TEXT PRIMARY KEY,
    asset_id TEXT,
    operation_id TEXT,
    host TEXT NOT NULL,
    user_name TEXT NOT NULL DEFAULT '',
    process TEXT NOT NULL DEFAULT '',
    arch TEXT NOT NULL DEFAULT '',
    os TEXT NOT NULL DEFAULT '',
    protocol TEXT NOT NULL DEFAULT 'http',
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'beacon', 'dormant', 'lost')),
    last_seen TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE event_rules (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    trigger TEXT NOT NULL,
    command TEXT NOT NULL,
    target TEXT NOT NULL DEFAULT 'all',
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    requested_by TEXT REFERENCES users(id),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE installed_services (
    id TEXT PRIMARY KEY,
    asset_id TEXT,
    operation_id TEXT,
    name TEXT NOT NULL,
    install_path TEXT NOT NULL DEFAULT '',
    running INTEGER NOT NULL DEFAULT 1 CHECK (running IN (0, 1)),
    started_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_callbacks_operation_id ON callbacks(operation_id);
CREATE INDEX idx_callbacks_last_seen ON callbacks(last_seen DESC);
CREATE INDEX idx_event_rules_created_at ON event_rules(created_at DESC);
CREATE INDEX idx_installed_services_operation_id ON installed_services(operation_id);
